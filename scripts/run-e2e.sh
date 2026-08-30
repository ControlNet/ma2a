#!/usr/bin/env sh
set -eu

runs=${MA2A_E2E_RUNS:-3}
output=${MA2A_E2E_OUTPUT:-target/e2e-evidence}
mkdir -p "$output"
printf 'runs=%s\n' "$runs" > "$output/summary.txt"

run=1
while [ "$run" -le "$runs" ]; do
  log="$output/run-$run.log"
  printf 'run=%s command=cargo nextest run -p ma2a-app --test e2e --profile ci\n' "$run" >> "$output/summary.txt"
  cargo nextest run -p ma2a-app --test e2e --profile ci --no-capture > "$log" 2>&1
  printf 'run=%s exit=0\n' "$run" >> "$output/summary.txt"
  run=$((run + 1))
done
