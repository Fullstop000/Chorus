#!/usr/bin/env bash
set -e

ROOT="$(cd "$(dirname "$0")" && pwd)"

# ── Colors ──
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
RESET='\033[0m'

echo -e "${CYAN}Chorus dev environment${RESET}"

port_in_use() {
  lsof -iTCP:"$1" -sTCP:LISTEN >/dev/null 2>&1
}

API_PORT=3001
if port_in_use "$API_PORT"; then
  echo -e "${YELLOW}⚠ Port :$API_PORT is already in use.${RESET}"
  if [ ! -t 0 ]; then
    echo "Run interactively to choose another API port."
    exit 1
  fi

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
