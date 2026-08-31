#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 3 ]]; then
  printf 'usage: %s ARCHIVE LICENSE_REPORT OUTPUT_SPDX\n' "$0" >&2
  exit 2
fi

archive=$1
report=$2
output=$3
temporary=$(mktemp -d)
trap 'rm -rf "$temporary"' EXIT

cargo metadata --locked --format-version 1 > "$temporary/cargo-metadata.json"
digest=$(awk 'NF >= 2 {print $1}' "${archive}.sha256")
created=${RELEASE_CREATED_AT:-1970-01-01T00:00:00Z}

jq -n \
  --arg name "$(basename "$archive")" \
  --arg digest "$digest" \
  --arg created "$created" \
  --slurpfile cargo "$temporary/cargo-metadata.json" \
  --slurpfile licenses "$report" '
  def rust_packages:
    $cargo[0].packages
    | to_entries
    | map({
        SPDXID: ("SPDXRef-Rust-" + (.key | tostring)),
        name: .value.name,
        versionInfo: .value.version,
        downloadLocation: (.value.source // "NOASSERTION"),
        filesAnalyzed: false,
        licenseConcluded: "NOASSERTION",
        licenseDeclared: (.value.license // "NOASSERTION"),
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
        licenseDeclared: .value.license,
        copyrightText: "NOASSERTION"
      });
  {
    spdxVersion: "SPDX-2.3",
    dataLicense: "CC0-1.0",
    SPDXID: "SPDXRef-DOCUMENT",
    name: $name,
    documentNamespace: ("https://github.com/ControlNet/ma2a/releases/sha256/" + $digest),
    creationInfo: {
      created: $created,
      creators: ["Tool: ma2a-generate-sbom"]
    },
    packages: (rust_packages + frontend_packages)
  }
' > "$output"
