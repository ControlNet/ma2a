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
# The tools name workspace crates and frontend packages by absolute path; the report names
# them relative to the checkout, so it does not depend on, or disclose, where it was built.
jq -n \
  --arg root "$PWD/" \
  --slurpfile rust "$temporary/rust.json" \
  --slurpfile frontend "$temporary/frontend.json" \
  'def relative: if type == "string" then (. / $root | join("")) else . end;
   def clean: walk(if type == "object" then with_entries(.key |= relative) else relative end);
   {schema_version: 1, rust: ($rust[0] | clean), frontend: ($frontend[0] | clean)}' > "$output"
