#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 4 ]]; then
  printf 'usage: %s ARCHIVE LICENSE_REPORT OUTPUT_SPDX TARGET\n' "$0" >&2
  exit 2
fi

archive=$1
report=$2
output=$3
target=$4
temporary=$(mktemp -d)
trap 'rm -rf "$temporary"' EXIT

cargo metadata --locked --format-version 1 --filter-platform "$target" \
  > "$temporary/cargo-metadata.json"
# The packages whose code the archive carries: `ma2a-app` and everything it reaches through
# normal dependency edges on the archive's target, without passing through a procedural macro,
# with features resolved for that build alone. A macro runs on the build host and only its
# expansion is compiled in, so it, and whatever only it depends on, is not listed; a library a
# macro shares with the program is still reached on its own path. Build and dev dependencies, the
# features only they turn on, and workspace tools such as xtask are not shipped. Each is named by
# name, version and whether it comes from crates.io, as Cargo prints a source only for a package
# from elsewhere, so a local crate with a shipped crate's name and version is not taken for it.
cargo tree --locked --color never -p ma2a-app --target "$target" -e normal,no-proc-macro \
  --prefix none --format '{p}' \
  | jq -Rn '[inputs | sub(" \\(\\*\\)$"; "")
      | (capture("^(?<name>[^ ]+) v(?<version>[^ ]+)(?<elsewhere> \\(.*\\))?$")
        // error("unexpected cargo tree line: \(.)"))
      | {key: "\(.name) \(.version) \(if .elsewhere then "elsewhere" else "crates.io" end)",
        value: true}]
    | from_entries' > "$temporary/shipped.json"
digest=$(awk 'NF >= 2 {print $1; exit}' "${archive}.sha256" | tr -d '\r')
[[ "$digest" =~ ^[0-9a-f]{64}$ ]] || {
  printf 'invalid SHA-256 digest in %s.sha256\n' "$archive" >&2
  exit 1
}
created=${RELEASE_CREATED_AT:-1970-01-01T00:00:00Z}
created=${created%$'\r'}
case "$created" in
  *+00:00) created="${created%+00:00}Z" ;;
  *-00:00) created="${created%-00:00}Z" ;;
esac
[[ "$created" =~ ^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z$ ]] || {
  printf 'RELEASE_CREATED_AT must be UTC ISO-8601 (YYYY-MM-DDThh:mm:ssZ): %s\n' "$created" >&2
  exit 1
}

jq -n -L "$(dirname "$0")" \
  --arg name "$(basename "$archive")" \
  --arg digest "$digest" \
  --arg created "$created" \
  --slurpfile cargo "$temporary/cargo-metadata.json" \
  --slurpfile shipped "$temporary/shipped.json" \
  --slurpfile licenses "$report" '
  include "spdx";
  def crates_io: .source == "registry+https://github.com/rust-lang/crates.io-index";
  def shipped_key: "\(.name) \(.version) \(if crates_io then "crates.io" else "elsewhere" end)";
  def rust_packages:
    [$cargo[0].packages[] | select($shipped[0][shipped_key])]
    | if (map(shipped_key) | unique | length) == ($shipped[0] | length)
        and length == ($shipped[0] | length) then .
      else error("the shipped Rust packages do not match the workspace metadata one to one") end
    | to_entries
    | map({
        SPDXID: ("SPDXRef-Rust-" + (.key | tostring)),
        name: .value.name,
        versionInfo: .value.version,
        downloadLocation: (
          if .value | crates_io then
            ("https://crates.io/crates/" + .value.name + "/" + .value.version)
          else
            "NOASSERTION"
          end
        ),
        filesAnalyzed: false,
        licenseConcluded: "NOASSERTION",
        licenseDeclared: (.value.license // "NOASSERTION" | spdx_declared),
        copyrightText: "NOASSERTION"
      });
  def frontend_packages:
    $licenses[0].frontend
    | to_entries
    | map(.key as $license | .value[] | .versions[] as $version | {
        license: $license,
        name: .name,
        version: $version
      })
    | unique_by([.name, .version])
    | to_entries
    | map({
        SPDXID: ("SPDXRef-Frontend-" + (.key | tostring)),
        name: .value.name,
        versionInfo: .value.version,
        downloadLocation: "NOASSERTION",
        filesAnalyzed: false,
        licenseConcluded: "NOASSERTION",
        licenseDeclared: (.value.license | spdx_declared),
        copyrightText: "NOASSERTION"
      });
  rust_packages as $rust |
  {
    spdxVersion: "SPDX-2.3",
    dataLicense: "CC0-1.0",
    SPDXID: "SPDXRef-DOCUMENT",
    name: $name,
    documentNamespace: ("https://github.com/ControlNet/ma2a/releases/sha256/" + $digest),
    documentDescribes: [$rust[] | select(.name == "ma2a-app") | .SPDXID],
    creationInfo: {
      created: $created,
      creators: ["Tool: ma2a-generate-sbom"]
    },
    packages: ($rust + frontend_packages)
  }
' > "$output"
