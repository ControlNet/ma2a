#!/usr/bin/env sh
set -eu

runs=${MA2A_E2E_RUNS:-3}
output=${MA2A_E2E_OUTPUT:-target/e2e-evidence}
junit=target/nextest/ci/junit.xml
mkdir -p "$output"
printf 'runs=%s\n' "$runs" > "$output/summary.txt"

run=1
while [ "$run" -le "$runs" ]; do
  log="$output/run-$run.log"
  evidence="$output/run-$run.jsonl"
  report="$output/run-$run.junit.xml"
  rm -f "$junit"
  printf 'run=%s command=cargo nextest run -p ma2a-app --test e2e --profile ci --success-output final --no-output-indent --color never\n' "$run" >> "$output/summary.txt"
  cargo nextest run -p ma2a-app --test e2e --profile ci --success-output final --no-output-indent --color never > "$log" 2>&1
  cp "$junit" "$report"
  grep -E '^\{' "$log" > "$evidence"
  for scenario in A B C D E F; do
    grep -q "\"scenario\":\"$scenario\"" "$evidence"
  done
  records=$(wc -l < "$evidence")
  printf 'run=%s exit=0 records=%s junit=%s evidence=%s\n' "$run" "$records" "$report" "$evidence" >> "$output/summary.txt"
  run=$((run + 1))
done
