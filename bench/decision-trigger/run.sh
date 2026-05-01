#!/usr/bin/env bash
# Decision-trigger benchmark — runs each case in an isolated agent in parallel,
# then classifies each agent's first reply turn as `decision` (dispatch_decision)
# or `chat` (send_message). Compares to the predicted column in cases.tsv.
#
# Usage: bench/decision-trigger/run.sh [server_url]
#
# Requires: chorus binary on PATH, server running, claude runtime authed.
set -euo pipefail

SERVER_URL="${1:-http://localhost:3001}"
BENCH_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# Cases file: defaults to cases.tsv (easy/smoke). Override with CASES=cases-hard.tsv.
CASES="${CASES:-$BENCH_DIR/cases.tsv}"
[ -f "$CASES" ] || CASES="$BENCH_DIR/$(basename "$CASES")"
# Runtime + model: which agent to spin up per case. Defaults are the cheapest
# stable combo. The matrix runner sets these per sweep.
RUNTIME="${RUNTIME:-claude}"
MODEL="${MODEL:-sonnet}"
RUN_LABEL="${RUN_LABEL:-${RUNTIME}-${MODEL}}"
RUN_ID="$(date +%s)-${RUN_LABEL}"
RESULTS_DIR="$BENCH_DIR/results/$RUN_ID"
mkdir -p "$RESULTS_DIR"

# Resolve chorus binary (prefer release, fall back to debug, then PATH).
CHORUS=""
if [ -x "$BENCH_DIR/../../target/release/chorus" ]; then
  CHORUS="$BENCH_DIR/../../target/release/chorus"
elif [ -x "$BENCH_DIR/../../target/debug/chorus" ]; then
  CHORUS="$BENCH_DIR/../../target/debug/chorus"
elif command -v chorus >/dev/null 2>&1; then
  CHORUS="chorus"
else
  echo "error: chorus binary not found. build with 'cargo build --bin chorus' first." >&2
  exit 1
fi

# Locate the server log so we can scrape tool calls per agent.
# Caller can override with CHORUS_LOG=/path/to/server.log.
LOG="${CHORUS_LOG:-/tmp/chorus-qa-server.log}"
if [ ! -f "$LOG" ]; then
  echo "warn: server log $LOG not found. set CHORUS_LOG to point to your server's stdout/stderr." >&2
  echo "      classification needs the log to scrape per-agent tool calls." >&2
  exit 1
fi

# Use the no-proxy env for curl since Chorus listens on localhost.
CURL=(curl --noproxy '*' -sS -m 10)

echo "== bench/decision-trigger run $RUN_ID =="
echo "  server:  $SERVER_URL"
echo "  log:     $LOG"
echo "  cases:   $CASES"
echo "  runtime: $RUNTIME"
echo "  model:   $MODEL"
echo "  out:     $RESULTS_DIR"

# Pause any non-bench agents so they don't flood the bench cohort with welcome
# messages during boot. We only stop running ones; KEEP_OTHERS=1 disables this.
declare -a PAUSED_AGENTS=()
if [ "${KEEP_OTHERS:-0}" != "1" ]; then
  while read -r name; do
    PAUSED_AGENTS+=("$name")
  done < <("${CURL[@]}" "$SERVER_URL/api/agents" | python3 -c "
import json, sys
d = json.load(sys.stdin)
for a in d:
    if a['name'].startswith('bench-dt-'):
        continue
    if a['status'] in ('ready', 'working'):
        print(a['name'])
")
  if [ ${#PAUSED_AGENTS[@]} -gt 0 ]; then
    echo
    echo "[0/5] pausing ${#PAUSED_AGENTS[@]} non-bench agents to keep #all quiet..."
    for a in "${PAUSED_AGENTS[@]}"; do
      "$CHORUS" agent stop --server-url "$SERVER_URL" "$a" >/dev/null 2>&1 || true
      echo "  stopped $a"
    done
  fi
fi

# Resume them on exit.
restore_agents() {
  if [ ${#PAUSED_AGENTS[@]} -eq 0 ]; then return; fi
  echo
  echo "restoring ${#PAUSED_AGENTS[@]} paused agents..."
  for a in "${PAUSED_AGENTS[@]}"; do
    "$CHORUS" agent start --server-url "$SERVER_URL" "$a" >/dev/null 2>&1 || true
  done
}
trap restore_agents EXIT

# 1) Read cases (skip header), spawn one agent per case.
# Chorus appends a hash suffix to the requested name, so we read the assigned
# name from `chorus agent create`'s log output instead of guessing.
echo
echo "[1/5] spawning agents..."
declare -a IDS PREDICTS PROMPTS AGENTS
while IFS=$'\t' read -r id predicted prompt; do
  [ "$id" = "id" ] && continue
  IDS+=("$id"); PREDICTS+=("$predicted"); PROMPTS+=("$prompt")
  base="bench-dt-${RUN_ID//[^a-zA-Z0-9]/-}-${id}"
  # Names can't be too long; runtime+model is appended for forensics in case-N description.
  out=$("$CHORUS" agent create \
    --runtime "$RUNTIME" --model "$MODEL" \
    --description "Decision-trigger bench, case $id, ${RUNTIME}/${MODEL}. Each DM is one independent test prompt." \
    --server-url "$SERVER_URL" \
    "$base" 2>&1)
  # Extract assigned name: "Agent @<name> created"
  agent_name=$(echo "$out" | grep -oE '@[A-Za-z0-9_-]+ created' | head -1 | sed 's/^@//;s/ created$//')
  if [ -z "$agent_name" ]; then
    echo "  failed to create $base; output:" >&2
    echo "$out" >&2
    exit 1
  fi
  AGENTS+=("$agent_name")
  echo "  spawned $agent_name (case $id, predicted=$predicted)"
done < "$CASES"

# 2) Wait for every agent to reach status=ready via API (avoids the
# intro-storm thundering herd in the log).
echo
echo "[2/5] waiting for agents to reach status=ready..."
deadline=$(( $(date +%s) + 300 ))
for agent in "${AGENTS[@]}"; do
  while :; do
    status=$("${CURL[@]}" "$SERVER_URL/api/agents" \
      | python3 -c "import json,sys; d=json.load(sys.stdin)
for a in d:
    if a['name']=='$agent': print(a['status']); break
" 2>/dev/null || true)
    case "$status" in
      ready|asleep|working) break ;;
    esac
    [ "$(date +%s)" -gt "$deadline" ] && { echo "  timeout waiting for $agent (status=$status)" >&2; exit 1; }
    sleep 2
  done
done
echo "  all ${#AGENTS[@]} agents ready"

# 3) Mark log line, dispatch all DMs in rapid sequence.
echo
echo "[3/5] dispatching cases..."
START_LINE=$(wc -l < "$LOG")
for n in "${!IDS[@]}"; do
  id="${IDS[$n]}"; agent="${AGENTS[$n]}"; prompt="${PROMPTS[$n]}"
  marker="[bench-dt case $id]"
  body="$marker $prompt"
  "$CHORUS" send "dm:@${agent}" "$body" --server-url "$SERVER_URL" >/dev/null 2>&1
  echo "  case $id → @$agent"
done

# 4) Wait for each agent to complete its case turn (next Natural after marker).
echo
echo "[4/5] waiting for case turns to complete..."
deadline=$(( $(date +%s) + 600 ))
declare -a DONE
for n in "${!IDS[@]}"; do DONE[$n]=0; done
remaining=${#IDS[@]}
while [ "$remaining" -gt 0 ]; do
  for n in "${!IDS[@]}"; do
    [ "${DONE[$n]}" = "1" ] && continue
    id="${IDS[$n]}"; agent="${AGENTS[$n]}"
    marker="\[bench-dt case $id\]"
    cur=$(wc -l < "$LOG")
    slice=$(sed -n "$((START_LINE+1)),${cur}p" "$LOG")
    if echo "$slice" | grep -qE "$marker" \
       && echo "$slice" | grep -q "${agent}.*reason=Natural"; then
      DONE[$n]=1
      remaining=$(( remaining - 1 ))
      echo "  case $id done ($remaining left)"
    fi
  done
  [ "$(date +%s)" -gt "$deadline" ] && { echo "  timeout"; break; }
  sleep 4
done

# Buffer for trailing tool-call logs.
sleep 5

# 5) Classify each case from the log slice and write results.
echo
echo "[5/5] classifying..."
RESULTS_TSV="$RESULTS_DIR/results.tsv"
echo -e "id\tagent\tpredicted\tactual\tmatch\tprompt" > "$RESULTS_TSV"
final_line=$(wc -l < "$LOG")
slice=$(sed -n "$((START_LINE+1)),${final_line}p" "$LOG")
match_count=0
total=${#IDS[@]}
for n in "${!IDS[@]}"; do
  id="${IDS[$n]}"; agent="${AGENTS[$n]}"; predicted="${PREDICTS[$n]}"; prompt="${PROMPTS[$n]}"
  agent_lines=$(echo "$slice" | grep -F "$agent" || true)
  # Look at log lines AFTER the marker arrived for this agent.
  if echo "$agent_lines" | grep -q "dispatch_decision"; then
    actual="decision"
  elif echo "$agent_lines" | grep -q "send_message"; then
    actual="chat"
  else
    actual="unknown"
  fi
  m="X"; [ "$actual" = "$predicted" ] && { m="OK"; match_count=$((match_count+1)); }
  short_prompt=$(echo "$prompt" | head -c 80)
  echo -e "${id}\t${agent}\t${predicted}\t${actual}\t${m}\t${short_prompt}" >> "$RESULTS_TSV"
done

echo
echo "== results =="
column -t -s$'\t' "$RESULTS_TSV"
echo
echo "match: $match_count/$total"

# Save log slice for forensics.
echo "$slice" > "$RESULTS_DIR/log-slice.txt"

# Cleanup unless KEEP_AGENTS=1.
if [ "${KEEP_AGENTS:-0}" = "1" ]; then
  echo
  echo "agents kept (KEEP_AGENTS=1):"
  for agent in "${AGENTS[@]}"; do echo "  $agent"; done
else
  echo
  echo "cleaning up agents..."
  for agent in "${AGENTS[@]}"; do
    "$CHORUS" agent delete --wipe --yes "$agent" --server-url "$SERVER_URL" >/dev/null 2>&1 || true
  done
fi

echo
echo "results: $RESULTS_TSV"
exit_code=0
[ "$match_count" -lt "$total" ] && exit_code=1
exit "$exit_code"
