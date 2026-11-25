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

# The synthetic checkout: a small workspace, and installed frontend packages written here.
fixture="$temporary/fixture"
mkdir -p "$fixture/web"
printf '[workspace]\nmembers = ["crates/ma2a-app", "crates/tool"]\nresolver = "2"\n' > "$fixture/Cargo.toml"
crate() {
  mkdir -p "$fixture/crates/$1/src"
  printf '[package]\nname = "%s"\nversion = "0.1.0"\nedition = "2021"\nlicense = "MIT"\n%s\n' \
    "$1" "$2" > "$fixture/crates/$1/Cargo.toml"
  touch "$fixture/crates/$1/src/lib.rs"
}
# Only the tests turn on the feature of shared that pulls in extra and common, so extra does not
# ship; common ships anyway, since the application also depends on it directly. The application
# ships itoa from crates.io, while a workspace tool uses a local crate with the same name and
# version and another license, which does not ship.
crate ma2a-app '[dependencies]
shared = { path = "../shared" }
common = { path = "../common" }
itoa = "=1.0.18"
[dev-dependencies]
shared = { path = "../shared", features = ["testing"] }'
printf 'fn main() {}\n' > "$fixture/crates/ma2a-app/src/main.rs"
crate shared '[dependencies]
extra = { path = "../extra", optional = true }
common = { path = "../common", optional = true }
[features]
testing = ["dep:extra", "dep:common"]'
crate extra ''
crate common ''
crate tool '[dependencies]
itoa = { path = "../itoa" }'
mkdir -p "$fixture/crates/itoa/src"
printf '[package]\nname = "itoa"\nversion = "1.0.18"\nedition = "2021"\nlicense = "ISC"\n' \
  > "$fixture/crates/itoa/Cargo.toml"
touch "$fixture/crates/itoa/src/lib.rs"
(cd "$fixture" && cargo generate-lockfile --offline --quiet)
package() {
  mkdir -p "$fixture/web/node_modules/$1"
  printf '%s\n' "$2" > "$fixture/web/node_modules/$1/package.json"
}
# A dependency also listed as optional is optional, so its absence is no error.
printf '%s\n' '{"dependencies": {"component": "1.0.0", "absent": "1.0.0", "legacy": "1.0.0",
  "unlicensed": "1.0.0", "spaced": "1.0.0", "filed": "1.0.0", "proprietary": "1.0.0",
  "compound": "1.0.0", "trailing": "1.0.0", "bare": "1.0.0", "dangling": "1.0.0",
  "pathed": "1.0.0"},
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
package dangling '{"name": "dangling", "version": "1.0.0", "license": "MIT / WITH LLVM-exception"}'
package pathed '{"name": "pathed", "version": "1.0.0", "license": "./LICENSE"}'
(cd "$fixture" && "$repo/scripts/generate-license-report.sh" "$temporary/fixture.json")
expected='{"Apache-2.0":[{"name":"renderer","versions":["1.0.0"]}],
  "MIT":[{"name":"component","versions":["1.0.0"]},{"name":"legacy","versions":["1.0.0"]}],
  "MIT / Apache-2.0":[{"name":"spaced","versions":["1.0.0"]}],
  "UNKNOWN":[{"name":"unlicensed","versions":["1.0.0"]}],
  "SEE LICENSE IN LICENSE.md":[{"name":"filed","versions":["1.0.0"]}],
  "UNLICENSED":[{"name":"proprietary","versions":["1.0.0"]}],
  "(MIT OR Apache-2.0) AND Apache-2.0 WITH LLVM-exception":[{"name":"compound","versions":["1.0.0"]}],
  "MIT /":[{"name":"trailing","versions":["1.0.0"]}],
  "/":[{"name":"bare","versions":["1.0.0"]}],
  "MIT / WITH LLVM-exception":[{"name":"dangling","versions":["1.0.0"]}],
  "./LICENSE":[{"name":"pathed","versions":["1.0.0"]}]}'
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
  "trailing":"MIT","bare":"NOASSERTION","dangling":"NOASSERTION","pathed":"NOASSERTION"}'
if ! jq -e --argjson expected "$expected" \
  '[.packages[] | select(.SPDXID | startswith("SPDXRef-Frontend-")) | {(.name): .licenseDeclared}]
   | add == $expected' "$temporary/fixture.spdx.json" >/dev/null; then
  printf 'the synthetic SBOM states frontend licenses wrongly:\n' >&2
  jq -S '[.packages[] | select(.SPDXID | startswith("SPDXRef-Frontend-"))]' \
    "$temporary/fixture.spdx.json" >&2
  exit 1
fi

# The SBOM lists the Rust packages the application's build compiles in, and no package that only
# the tests or a workspace tool reach, even one with a shipped package's name and version.
if ! jq -e '[.packages[] | select(.SPDXID | startswith("SPDXRef-Rust-"))
    | {name, licenseDeclared, downloadLocation}] | sort_by(.name)
    == [{name: "common", licenseDeclared: "MIT", downloadLocation: "NOASSERTION"},
      {name: "itoa", licenseDeclared: "MIT OR Apache-2.0",
        downloadLocation: "https://crates.io/crates/itoa/1.0.18"},
      {name: "ma2a-app", licenseDeclared: "MIT", downloadLocation: "NOASSERTION"},
      {name: "shared", licenseDeclared: "MIT", downloadLocation: "NOASSERTION"}]' \
    "$temporary/fixture.spdx.json" >/dev/null; then
  printf 'the synthetic SBOM lists the wrong Rust packages:\n' >&2
  jq -S '[.packages[] | select(.SPDXID | startswith("SPDXRef-Rust-"))]' \
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
