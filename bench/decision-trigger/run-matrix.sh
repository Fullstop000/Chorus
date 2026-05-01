#!/usr/bin/env bash
# Run the decision-trigger bench across multiple (runtime, model) combos and
# collate into a single matrix. Reads models from bench/decision-trigger/models.tsv.
#
# Usage:
#   ./bench/decision-trigger/run-matrix.sh                          # default: cases.tsv, all models in models.tsv
#   CASES=cases-hard.tsv ./bench/decision-trigger/run-matrix.sh
#   MODELS=/path/to/custom-models.tsv ./bench/decision-trigger/run-matrix.sh
#
# Output:
#   bench/decision-trigger/results/matrix-<unix_ts>/
#     matrix.tsv                                     — case x model match grid
#     <runtime>-<model>/results.tsv                  — per-model raw results
set -euo pipefail

BENCH_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SERVER_URL="${1:-http://localhost:3001}"
MODELS="${MODELS:-$BENCH_DIR/models.tsv}"
CASES="${CASES:-$BENCH_DIR/cases.tsv}"
[ -f "$MODELS" ] || { echo "models file not found: $MODELS" >&2; exit 1; }
[ -f "$CASES" ] || { echo "cases file not found: $CASES" >&2; exit 1; }

MATRIX_RUN_ID=$(date +%s)
OUT_DIR="$BENCH_DIR/results/matrix-${MATRIX_RUN_ID}"
mkdir -p "$OUT_DIR"

echo "== bench/decision-trigger MATRIX run $MATRIX_RUN_ID =="
echo "  models: $MODELS"
echo "  cases:  $CASES"
echo "  server: $SERVER_URL"
echo "  out:    $OUT_DIR"
echo

# Read model rows (skip header).
declare -a RUNTIMES MODELS_LIST LABELS TIERS
while IFS=$'\t' read -r runtime model tier label; do
  [ "$runtime" = "runtime" ] && continue
  [ -z "$runtime" ] && continue
  RUNTIMES+=("$runtime"); MODELS_LIST+=("$model"); TIERS+=("$tier"); LABELS+=("$label")
done < "$MODELS"

if [ ${#RUNTIMES[@]} -eq 0 ]; then
  echo "no models in $MODELS" >&2; exit 1
fi

echo "matrix has ${#RUNTIMES[@]} models:"
for n in "${!RUNTIMES[@]}"; do
  echo "  ${LABELS[$n]} (${TIERS[$n]}, ${RUNTIMES[$n]}/${MODELS_LIST[$n]})"
done
echo

# Run the bench once per model.
declare -a MODEL_RESULT_PATHS
for n in "${!RUNTIMES[@]}"; do
  runtime="${RUNTIMES[$n]}"; model="${MODELS_LIST[$n]}"; label="${LABELS[$n]}"
  echo "----- [$((n+1))/${#RUNTIMES[@]}] $label ($runtime/$model) -----"
  RUNTIME="$runtime" MODEL="$model" RUN_LABEL="$label" CASES="$CASES" \
    bash "$BENCH_DIR/run.sh" "$SERVER_URL" \
    > "$OUT_DIR/${label}.log" 2>&1 || {
      echo "  $label run failed; continuing matrix"
    }
  # Find the per-run results.tsv this run produced.
  result_path=$(ls -t "$BENCH_DIR/results/" 2>/dev/null | grep -E "^[0-9]+-${label}\$" | head -1)
  if [ -n "$result_path" ] && [ -f "$BENCH_DIR/results/$result_path/results.tsv" ]; then
    cp "$BENCH_DIR/results/$result_path/results.tsv" "$OUT_DIR/${label}-results.tsv"
    MODEL_RESULT_PATHS+=("$OUT_DIR/${label}-results.tsv")
    score=$(awk -F'\t' 'NR>1 && $5=="OK"' "$OUT_DIR/${label}-results.tsv" | wc -l | tr -d ' ')
    total=$(awk -F'\t' 'NR>1' "$OUT_DIR/${label}-results.tsv" | wc -l | tr -d ' ')
    echo "  $label: $score/$total"
  else
    echo "  $label: no results.tsv"
    MODEL_RESULT_PATHS+=("")
  fi
  echo
done

# Build the matrix table.
MATRIX="$OUT_DIR/matrix.tsv"
{
  printf "case\tpredicted"
  for label in "${LABELS[@]}"; do printf "\t%s" "$label"; done
  printf "\tprompt\n"

  # Read cases (id, predicted, prompt).
  while IFS=$'\t' read -r id predicted prompt; do
    [ "$id" = "id" ] && continue
    [ -z "$id" ] && continue
    short=$(echo "$prompt" | head -c 80)
    printf "%s\t%s" "$id" "$predicted"
    for n in "${!LABELS[@]}"; do
      rp="${MODEL_RESULT_PATHS[$n]}"
      if [ -z "$rp" ] || [ ! -f "$rp" ]; then
        printf "\t-"
        continue
      fi
      # Find this case's row in this model's results.
      row=$(awk -F'\t' -v id="$id" 'NR>1 && $1==id {print $4 "/" $5}' "$rp" | head -1)
      printf "\t%s" "${row:-?}"
    done
    printf "\t%s\n" "$short"
  done < "$CASES"
} > "$MATRIX"

echo "===== MATRIX ====="
column -t -s$'\t' "$MATRIX"
echo
echo "summary:"
for n in "${!LABELS[@]}"; do
  rp="${MODEL_RESULT_PATHS[$n]}"
  if [ -z "$rp" ] || [ ! -f "$rp" ]; then
    echo "  ${LABELS[$n]}: no data"
    continue
  fi
  score=$(awk -F'\t' 'NR>1 && $5=="OK"' "$rp" | wc -l | tr -d ' ')
  total=$(awk -F'\t' 'NR>1' "$rp" | wc -l | tr -d ' ')
  echo "  ${LABELS[$n]} (${TIERS[$n]}): $score/$total"
done
echo
echo "matrix saved to $MATRIX"
