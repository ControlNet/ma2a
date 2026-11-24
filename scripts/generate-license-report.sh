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
cargo metadata --locked --no-deps --format-version 1 > "$temporary/metadata.json"
# The tools name workspace crates and frontend packages by absolute path; the report names
# them relative to the checkout, so it does not depend on, or disclose, where it was built.
# The checkout has several spellings: the shell's logical path, the physical path the tools
# resolve, Cargo's native workspace root, and the percent-encoded path in Cargo's file URLs.
# Cargo's URL root is read from a package ID rather than encoded here, so it matches exactly.
jq -n \
  --arg logical "$PWD" \
  --arg physical "$(pwd -P)" \
  --slurpfile metadata "$temporary/metadata.json" \
  --slurpfile rust "$temporary/rust.json" \
  --slurpfile frontend "$temporary/frontend.json" \
  '($metadata[0].workspace_root) as $native
   | ($metadata[0].packages[0]) as $package
   | ($package.manifest_path | rtrimstr("Cargo.toml") | ltrimstr($native)
      | gsub("\\\\"; "/") | rtrimstr("/")) as $member
   | ($package.id | split("#")[0] | ltrimstr("path+file://")) as $member_url
   | if ($member_url | endswith($member)) then . else error("unexpected Cargo package ID") end
   | [$logical, $physical, $native, ($member_url | rtrimstr($member))] as $roots
   | def relative:
       if type == "string"
       then reduce $roots[] as $root (.; (. / ($root + "/") | join("")) | (. / ($root + "\\") | join("")))
       else . end;
     def clean: walk(if type == "object" then with_entries(.key |= relative) else relative end);
     {schema_version: 1, rust: ($rust[0] | clean), frontend: ($frontend[0] | clean),
      roots: $roots}' > "$temporary/report.json"

# Fail instead of publishing a report that still names the checkout.
jq -e '.roots as $roots | del(.roots) | [.. | strings, (.. | objects | keys[])]
  | all(. as $text | $roots | all(. as $root | $text | contains($root) | not))' \
  "$temporary/report.json" > /dev/null || {
  printf 'license report still contains the checkout path\n' >&2
  exit 1
}
jq 'del(.roots)' "$temporary/report.json" > "$output"
