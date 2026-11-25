#!/usr/bin/env bash
# Runs the license report generator from checkouts whose paths differ, and checks that the
# report never names the checkout and is the same wherever it was built. Then runs it, and the
# SBOM generator, on a small synthetic checkout whose frontend packages use the dependency and
# license forms the real ones do not.
set -euo pipefail

repo=$(git rev-parse --show-toplevel)
cd "$repo"
temporary=$(mktemp -d)
checkout="$temporary/spaced checkout"
# A checkout whose path ends with the whole path of a symlink to it.
nested="$temporary/outer$temporary/suffix"
cleanup() {
  git worktree remove --force "$checkout" 2>/dev/null || true
  git worktree remove --force "$nested" 2>/dev/null || true
  rm -rf "$temporary"
}
trap cleanup EXIT

./scripts/generate-license-report.sh "$temporary/plain.json"

# A logical path through a symlink differs from the physical path the tools print.
ln -s "$repo" "$temporary/linked checkout"
(cd "$temporary/linked checkout" && ./scripts/generate-license-report.sh "$temporary/linked.json")

# A physical path with a space is percent-encoded in Cargo's file URLs.
git worktree add --quiet --detach "$checkout" HEAD
(cd "$checkout/web" && bun install --frozen-lockfile --silent)
(cd "$checkout" && "$repo/scripts/generate-license-report.sh" "$temporary/spaced.json")

# A logical path that is a suffix of the physical one must not be cut out of the middle of it.
git worktree add --quiet --detach "$nested" HEAD
(cd "$nested/web" && bun install --frozen-lockfile --silent)
ln -s "$nested" "$temporary/suffix"
(cd "$temporary/suffix" && "$repo/scripts/generate-license-report.sh" "$temporary/suffix.json")

for report in linked spaced suffix; do
  if grep -qF -e "$temporary" -e "${temporary// /%20}" -e "$repo" "$temporary/$report.json"; then
    printf 'the %s report names its checkout\n' "$report" >&2
    exit 1
  fi
  if ! cmp -s <(jq -S . "$temporary/plain.json") <(jq -S . "$temporary/$report.json"); then
    printf 'the %s report differs from the plain one\n' "$report" >&2
    exit 1
  fi
done
printf 'license report is independent of the checkout path\n'

# The synthetic checkout: a one-crate workspace, and installed frontend packages written here.
fixture="$temporary/fixture"
mkdir -p "$fixture/crates/ma2a-app/src" "$fixture/web"
printf '[workspace]\nmembers = ["crates/ma2a-app"]\nresolver = "2"\n' > "$fixture/Cargo.toml"
printf '[package]\nname = "ma2a-app"\nversion = "0.1.0"\nedition = "2021"\nlicense = "MIT"\n' \
  > "$fixture/crates/ma2a-app/Cargo.toml"
printf 'fn main() {}\n' > "$fixture/crates/ma2a-app/src/main.rs"
(cd "$fixture" && cargo generate-lockfile --offline --quiet)
package() {
  mkdir -p "$fixture/web/node_modules/$1"
  printf '%s\n' "$2" > "$fixture/web/node_modules/$1/package.json"
}
# A dependency also listed as optional is optional, so its absence is no error.
printf '%s\n' '{"dependencies": {"component": "1.0.0", "absent": "1.0.0", "legacy": "1.0.0",
  "unlicensed": "1.0.0", "spaced": "1.0.0", "filed": "1.0.0", "proprietary": "1.0.0",
  "compound": "1.0.0", "trailing": "1.0.0", "bare": "1.0.0"},
  "optionalDependencies": {"absent": "1.0.0"}}' \
  > "$fixture/web/package.json"
# The component ships its peer, the renderer, and does without an optional peer that is absent.
package component '{"name": "component", "version": "1.0.0", "license": "MIT",
  "peerDependencies": {"renderer": "1.0.0", "plugin": "1.0.0"},
  "peerDependenciesMeta": {"plugin": {"optional": true}}}'
package renderer '{"name": "renderer", "version": "1.0.0", "license": "Apache-2.0"}'
package legacy '{"name": "legacy", "version": "1.0.0", "license": "",
  "licenses": [{"type": "MIT"}, {"type": ""}, {"url": "https://example.com/license"}]}'
package unlicensed '{"name": "unlicensed", "version": "1.0.0", "license": {"type": " "}}'
package spaced '{"name": "spaced", "version": "1.0.0", "license": "MIT / Apache-2.0"}'
# npm also allows a license file, UNLICENSED and stray slashes, none of them an SPDX expression.
package filed '{"name": "filed", "version": "1.0.0", "license": "SEE LICENSE IN LICENSE.md"}'
package proprietary '{"name": "proprietary", "version": "1.0.0", "license": "UNLICENSED"}'
package compound '{"name": "compound", "version": "1.0.0",
  "license": "(MIT OR Apache-2.0) AND Apache-2.0 WITH LLVM-exception"}'
package trailing '{"name": "trailing", "version": "1.0.0", "license": "MIT / "}'
package bare '{"name": "bare", "version": "1.0.0", "license": "/"}'
(cd "$fixture" && "$repo/scripts/generate-license-report.sh" "$temporary/fixture.json")
expected='{"Apache-2.0":[{"name":"renderer","versions":["1.0.0"]}],
  "MIT":[{"name":"component","versions":["1.0.0"]},{"name":"legacy","versions":["1.0.0"]}],
  "MIT / Apache-2.0":[{"name":"spaced","versions":["1.0.0"]}],
  "UNKNOWN":[{"name":"unlicensed","versions":["1.0.0"]}],
  "SEE LICENSE IN LICENSE.md":[{"name":"filed","versions":["1.0.0"]}],
  "UNLICENSED":[{"name":"proprietary","versions":["1.0.0"]}],
  "(MIT OR Apache-2.0) AND Apache-2.0 WITH LLVM-exception":[{"name":"compound","versions":["1.0.0"]}],
  "MIT /":[{"name":"trailing","versions":["1.0.0"]}],
  "/":[{"name":"bare","versions":["1.0.0"]}]}'
if ! jq -e --argjson expected "$expected" '.frontend == $expected' "$temporary/fixture.json" \
  >/dev/null; then
  printf 'the synthetic frontend report is wrong:\n' >&2
  jq -S .frontend "$temporary/fixture.json" >&2
  exit 1
fi

# The SBOM states a slashed license as an SPDX expression, keeps a valid expression, and states
# anything else, unknown or not an SPDX expression, as NOASSERTION.
printf 'archive' > "$temporary/fixture.tar.xz"
printf '%s *fixture.tar.xz\n' "$(printf '0%.0s' $(seq 64))" > "$temporary/fixture.tar.xz.sha256"
(cd "$fixture" && "$repo/scripts/generate-sbom.sh" "$temporary/fixture.tar.xz" \
  "$temporary/fixture.json" "$temporary/fixture.spdx.json" x86_64-unknown-linux-gnu)
expected='{"component":"MIT","legacy":"MIT","renderer":"Apache-2.0",
  "spaced":"MIT OR Apache-2.0","unlicensed":"NOASSERTION","filed":"NOASSERTION",
  "proprietary":"NOASSERTION","compound":"(MIT OR Apache-2.0) AND Apache-2.0 WITH LLVM-exception",
  "trailing":"MIT","bare":"NOASSERTION"}'
if ! jq -e --argjson expected "$expected" \
  '[.packages[] | select(.SPDXID | startswith("SPDXRef-Frontend-")) | {(.name): .licenseDeclared}]
   | add == $expected' "$temporary/fixture.spdx.json" >/dev/null; then
  printf 'the synthetic SBOM states frontend licenses wrongly:\n' >&2
  jq -S '[.packages[] | select(.SPDXID | startswith("SPDXRef-Frontend-"))]' \
    "$temporary/fixture.spdx.json" >&2
  exit 1
fi

# A required package that is not installed fails the report instead of leaving it out.
mv "$fixture/web/node_modules/renderer" "$temporary/renderer"
if (cd "$fixture" && "$repo/scripts/generate-license-report.sh" "$temporary/missing.json") \
  >/dev/null 2>&1; then
  printf 'a report missing a required peer unexpectedly succeeded\n' >&2
  exit 1
fi
printf 'license report and SBOM read every frontend dependency and license form\n'
