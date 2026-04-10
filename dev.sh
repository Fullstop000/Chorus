#!/usr/bin/env bash
set -e

ROOT="$(cd "$(dirname "$0")" && pwd)"

# ── Colors ──
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
RESET='\033[0m'

echo -e "${CYAN}Chorus dev environment${RESET}"

# ── Auto-install ACP adapter binaries when the base runtime is present ──
# Uses the user's interactive shell to find npm, so this works regardless
# of which node version manager (nvm, volta, fnm, system) they use.
_npm=""
if command -v npm >/dev/null 2>&1; then
  _npm="npm"
else
  _shell="${SHELL:-/bin/sh}"
  _npm_bin="$("$_shell" -i -c 'command -v npm 2>/dev/null' 2>/dev/null | tr -d '[:space:]')"
  if [ -n "$_npm_bin" ]; then
    _npm="$_npm_bin"
  fi
fi

install_acp_adapter() {
  local binary="$1" package="$2" runtime="$3"
  if command -v "$runtime" >/dev/null 2>&1 && ! command -v "$binary" >/dev/null 2>&1; then
    if [ -n "$_npm" ]; then
      echo -e "${YELLOW}▶ Installing $binary (ACP adapter for $runtime)...${RESET}"
      "$_npm" install -g "$package"
    else
      echo -e "${YELLOW}⚠ $binary not found. Install it with: npm install -g $package${RESET}"
    fi
  fi
}

install_acp_adapter "codex-acp" "@zed-industries/codex-acp" "codex"

port_in_use() {
  lsof -iTCP:"$1" -sTCP:LISTEN >/dev/null 2>&1
}

API_PORT=3001

pick_alternate_port() {
  while true; do
    printf "Enter a different API port, or type 'q' to abort: "
    read -r CHOSEN_PORT
    if [ "$CHOSEN_PORT" = "q" ] || [ "$CHOSEN_PORT" = "Q" ]; then
      echo "Aborted."
      exit 1
    fi
    if ! [[ "$CHOSEN_PORT" =~ ^[0-9]+$ ]] || [ "$CHOSEN_PORT" -lt 1 ] || [ "$CHOSEN_PORT" -gt 65535 ]; then
      echo "Port must be a number between 1 and 65535."
      continue
    fi
    if port_in_use "$CHOSEN_PORT"; then
      echo "Port :$CHOSEN_PORT is also in use."
      continue
    fi
    API_PORT="$CHOSEN_PORT"
    break
  done
}

if port_in_use "$API_PORT"; then
  PORT_PID=$(lsof -iTCP:"$API_PORT" -sTCP:LISTEN -t 2>/dev/null | head -1)
  IS_CHORUS=false
  if [ -n "$PORT_PID" ]; then
    PROC_CMD=$(ps -o comm= -p "$PORT_PID" 2>/dev/null | xargs basename 2>/dev/null)
    if [ "$PROC_CMD" = "chorus" ]; then
      IS_CHORUS=true
    fi
  fi

  if [ "$IS_CHORUS" = true ]; then
    echo -e "${YELLOW}⚠ Port :$API_PORT is in use by a Chorus backend process (pid $PORT_PID).${RESET}"
    if [ ! -t 0 ]; then
      # Non-interactive: kill the old Chorus process automatically.
      kill "$PORT_PID" 2>/dev/null
      sleep 0.3
    else
      printf "Kill it and restart? [Y/n/q] "
      read -r ANSWER
      case "$ANSWER" in
        q|Q)
          echo "Aborted."
          exit 1
          ;;
        n|N)
          pick_alternate_port
          ;;
        *)
          kill "$PORT_PID" 2>/dev/null
          sleep 0.3
          ;;
      esac
    fi
  else
    echo -e "${YELLOW}⚠ Port :$API_PORT is already in use by another process (pid ${PORT_PID:-unknown}).${RESET}"
    if [ ! -t 0 ]; then
      echo "Run interactively to choose another API port."
      exit 1
    fi
    pick_alternate_port
  fi
fi

# ── Build Rust binary if needed ──
echo -e "${YELLOW}▶ Building chorus...${RESET}"
cd "$ROOT"
cargo build 2>&1 | grep -E "^error|Compiling chorus|Finished" || true

# ── Start Rust server ──
echo -e "${YELLOW}▶ Starting chorus server on :$API_PORT...${RESET}"
"$ROOT/target/debug/chorus" serve --port "$API_PORT" &
CHORUS_PID=$!

# ── Wait for server to be ready ──
for i in $(seq 1 20); do
  if curl -sf "http://localhost:$API_PORT/api/whoami" > /dev/null 2>&1; then
    break
  fi
  sleep 0.2
done
echo -e "${GREEN}✓ chorus server ready (pid $CHORUS_PID)${RESET}"

# ── Install UI deps if needed ──
if [ ! -d "$ROOT/ui/node_modules" ]; then
  echo -e "${YELLOW}▶ Installing UI dependencies...${RESET}"
  cd "$ROOT/ui" && npm install
fi

# ── Start Vite dev server ──
echo -e "${YELLOW}▶ Starting Vite dev server on :5173...${RESET}"
cd "$ROOT/ui"
CHORUS_API_PORT="$API_PORT" npm run dev &
VITE_PID=$!

echo ""
echo -e "${GREEN}✓ Dev environment running${RESET}"
echo -e "  UI  → ${CYAN}http://localhost:5173${RESET}"
echo -e "  API → ${CYAN}http://localhost:$API_PORT${RESET}"
echo ""
echo "Press Ctrl+C to stop all processes."

# ── Cleanup on exit ──
trap 'echo ""; echo "Stopping..."; kill $CHORUS_PID $VITE_PID 2>/dev/null; exit 0' INT TERM

wait
