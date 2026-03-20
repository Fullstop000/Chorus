#!/usr/bin/env bash
set -e

ROOT="$(cd "$(dirname "$0")" && pwd)"

# ── Colors ──
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
RESET='\033[0m'

echo -e "${CYAN}Chorus dev environment${RESET}"

# ── Build Rust binary if needed ──
echo -e "${YELLOW}▶ Building chorus...${RESET}"
cd "$ROOT"
cargo build 2>&1 | grep -E "^error|Compiling chorus|Finished" || true

# ── Start Rust server ──
echo -e "${YELLOW}▶ Starting chorus server on :3001...${RESET}"
"$ROOT/target/debug/chorus" serve --port 3001 &
CHORUS_PID=$!

# ── Wait for server to be ready ──
for i in $(seq 1 20); do
  if curl -sf http://localhost:3001/api/whoami > /dev/null 2>&1; then
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
npm run dev &
VITE_PID=$!

echo ""
echo -e "${GREEN}✓ Dev environment running${RESET}"
echo -e "  UI  → ${CYAN}http://localhost:5173${RESET}"
echo -e "  API → ${CYAN}http://localhost:3001${RESET}"
echo ""
echo "Press Ctrl+C to stop all processes."

# ── Cleanup on exit ──
trap 'echo ""; echo "Stopping..."; kill $CHORUS_PID $VITE_PID 2>/dev/null; exit 0' INT TERM

wait
