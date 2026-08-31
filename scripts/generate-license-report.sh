#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  printf 'usage: %s OUTPUT_JSON\n' "$0" >&2
  exit 2
fi

output=$1
temporary=$(mktemp -d)
trap 'rm -rf "$temporary"' EXIT

cargo deny list --format json --layout crate > "$temporary/rust.json"
(cd web && bun pm licenses --json --prod) > "$temporary/frontend.json"
jq -n \
  --slurpfile rust "$temporary/rust.json" \
  --slurpfile frontend "$temporary/frontend.json" \
  '{schema_version: 1, rust: $rust[0], frontend: $frontend[0]}' > "$output"
