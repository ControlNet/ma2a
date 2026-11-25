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
# The pinned Bun has no license listing, so the production dependency closure is read from the
# installed packages, resolving each dependency the way Node does, and grouped by license. An
# optional dependency counts when it is installed, since it then ships like any other. The script
# is a module: the pinned Bun exits 0 without a word when a CommonJS script given to --eval throws.
# A package is resolved from where it really is, past any link to it, as Node does: Bun's isolated
# layout links each package into node_modules and installs its dependencies beside the real one.
(cd web && bun --eval "$(cat <<'JS'
import { existsSync, readFileSync, realpathSync } from "node:fs";
import { dirname, join } from "node:path";
const manifest = (dir) => JSON.parse(readFileSync(join(dir, "package.json"), "utf8"));
const root = realpathSync(process.cwd());
const resolve = (from, name, optional) => {
  for (let dir = from; ; dir = dirname(dir)) {
    const candidate = join(dir, "node_modules", name);
    if (existsSync(join(candidate, "package.json"))) return realpathSync(candidate);
    if (dir === root || dir === dirname(dir)) {
      if (optional) return undefined;
      throw new Error(`${name} is not installed`);
    }
  }
};
// A license is an SPDX string, a legacy {type} object, or a legacy list of either. A blank or
// malformed entry names no license.
const named = (value) => {
  const name = typeof value === "string" ? value : value?.type;
  return typeof name === "string" ? name.trim() : "";
};
const license = ({ license, licenses }) =>
  named(license)
  || (Array.isArray(licenses) ? licenses.map(named).filter(Boolean).join(" OR ") : "")
  || "UNKNOWN";
const seen = new Map();
const visit = (from, name, optional = false) => {
  const dir = resolve(from, name, optional);
  if (dir === undefined || seen.has(dir)) return;
  const pkg = manifest(dir);
  seen.set(dir, pkg);
  dependencies(dir, pkg);
};
// A dependency also listed as optional is optional. A peer is imported by the package that
// names it, so it ships too; one marked optional counts only when installed.
const dependencies = (dir, pkg) => {
  const optional = pkg.optionalDependencies ?? {};
  const peers = pkg.peerDependenciesMeta ?? {};
  for (const dep of Object.keys(pkg.dependencies ?? {})) visit(dir, dep, Object.hasOwn(optional, dep));
  for (const dep of Object.keys(optional)) visit(dir, dep, true);
  for (const dep of Object.keys(pkg.peerDependencies ?? {})) {
    visit(dir, dep, peers[dep]?.optional === true);
  }
};
dependencies(root, manifest("."));
const report = {};
for (const pkg of seen.values()) {
  const group = (report[license(pkg)] ??= []);
  const entry = group.find((known) => known.name === pkg.name);
  if (entry) entry.versions = [...new Set([...entry.versions, pkg.version])].sort();
  else group.push({ name: pkg.name, versions: [pkg.version] });
}
for (const group of Object.values(report)) group.sort((a, b) => a.name.localeCompare(b.name));
console.log(JSON.stringify(report));
JS
)") > "$temporary/frontend.json"
cargo metadata --locked --no-deps --format-version 1 > "$temporary/metadata.json"
# Cargo names workspace crates by absolute path; the report names them relative to the
# checkout, so it does not depend on, or disclose, where it was built.
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
   | [$logical, $physical, $native, ($member_url | rtrimstr($member))]
   | unique | sort_by(length) | reverse | . as $roots
   | def relative:
       if type == "string"
       then reduce ($roots[] + ("/", "\\")) as $root (.;
         (. / ("file://" + $root) | join("file://"))
         | if startswith($root) then .[($root | length):] else . end)
       else . end;
     def clean: walk(if type == "object" then with_entries(.key |= relative) else relative end);
     {schema_version: 1, rust: ($rust[0] | clean), frontend: ($frontend[0] | clean),
      roots: $roots}' > "$temporary/report.json"

# Fail instead of publishing a report that still names the checkout where a path can stand.
jq -e '.roots as $roots | del(.roots) | [.. | strings, (.. | objects | keys[])]
  | all(. as $text | $roots
    | all(. as $root | $text | (startswith($root) or contains("file://" + $root)) | not))' \
  "$temporary/report.json" > /dev/null || {
  printf 'license report still contains the checkout path\n' >&2
  exit 1
}
jq 'del(.roots)' "$temporary/report.json" > "$output"
