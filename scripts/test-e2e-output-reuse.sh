#!/usr/bin/env bash
# Checks which existing output directories scripts/run-e2e.sh reuses, and that it removes only
# the files it writes. A stand-in `cargo` that always fails stops the script right after that
# check, so no test runs.
set -euo pipefail

temporary=$(mktemp -d)
trap 'rm -rf "$temporary"' EXIT
mkdir "$temporary/bin"
printf '#!/bin/sh\nexit 1\n' > "$temporary/bin/cargo"
chmod +x "$temporary/bin/cargo"

# Runs the script against output directory $1, with run count $2 (default 1), and prints its
# exit status.
run() {
  local status=0
  PATH="$temporary/bin:$PATH" MA2A_E2E_OUTPUT="$1" MA2A_E2E_RUNS="${2:-1}" \
    ./scripts/run-e2e.sh >/dev/null 2>&1 || status=$?
  printf '%s' "$status"
}

# A run count must be a positive integer without leading zeros, and is checked before the
# output directory is created.
for runs in 0 00 01 x 1x; do
  test "$(run "$temporary/count-$runs" "$runs")" -eq 2 \
    || { printf 'MA2A_E2E_RUNS=%s was accepted\n' "$runs" >&2; exit 1; }
  test ! -e "$temporary/count-$runs" \
    || { printf 'MA2A_E2E_RUNS=%s created the output\n' "$runs" >&2; exit 1; }
done
for runs in 1 3 10; do
  test "$(run "$temporary/count-$runs" "$runs")" -ne 2 \
    || { printf 'MA2A_E2E_RUNS=%s was refused\n' "$runs" >&2; exit 1; }
done

# An evidence directory left by an interrupted run is reused, and only its files go.
reused="$temporary/reused"
mkdir "$reused"
for name in .ma2a-e2e-evidence summary.txt run-1.log run-12.junit.xml run-1.tests.txt.unsorted \
  inventory.txt.unsorted; do
  printf 'old' > "$reused/$name"
done
status=$(run "$reused")
test "$status" -ne 2 || { printf 'an interrupted evidence directory was refused\n' >&2; exit 1; }
for name in summary.txt run-1.log run-12.junit.xml run-1.tests.txt.unsorted; do
  test ! -e "$reused/$name" || { printf '%s was kept\n' "$name" >&2; exit 1; }
done

# The same evidence directory is reused whether or not its name ends in a separator.
for spelling in "$temporary/slash/" "$temporary/slash//" "$temporary/slash"; do
  mkdir -p "$temporary/slash"
  printf '' > "$temporary/slash/.ma2a-e2e-evidence"
  printf 'old' > "$temporary/slash/run-1.log"
  test "$(run "$spelling")" -ne 2 \
    || { printf '%s was refused\n' "$spelling" >&2; exit 1; }
  test ! -e "$temporary/slash/run-1.log" \
    || { printf '%s kept run-1.log\n' "$spelling" >&2; exit 1; }
done
printf 'notes' > "$temporary/slash/notes.txt"
test "$(run "$temporary/slash/")" -eq 2
test "$(cat "$temporary/slash/notes.txt")" = notes

# A file merely named like the script's own is kept, and the directory is refused.
near="$temporary/near"
mkdir "$near"
printf '' > "$near/.ma2a-e2e-evidence"
printf 'notes' > "$near/run-1-notes.log"
printf 'old' > "$near/run-1.log"
test "$(run "$near")" -eq 2
test "$(cat "$near/run-1-notes.log")" = notes
test "$(cat "$near/run-1.log")" = old

# Without the marker, nothing is touched.
unmarked="$temporary/unmarked"
mkdir "$unmarked"
printf 'mine' > "$unmarked/summary.txt"
test "$(run "$unmarked")" -eq 2
test "$(cat "$unmarked/summary.txt")" = mine

printf 'run-e2e output reuse: ok\n'
