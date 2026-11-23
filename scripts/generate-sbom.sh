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

# Resolved for the archive's target only, so another platform's dependencies never appear.
cargo metadata --locked --format-version 1 --filter-platform "$target" \
  > "$temporary/cargo-metadata.json"
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

jq -n \
  --arg name "$(basename "$archive")" \
  --arg digest "$digest" \
  --arg created "$created" \
  --slurpfile cargo "$temporary/cargo-metadata.json" \
  --slurpfile licenses "$report" '
  # The packages whose code the archive carries: `ma2a-app` and everything it reaches through
  # normal dependency edges, without passing through a procedural macro. A macro runs on the
  # build host and only its expansion is compiled in, so it, and whatever only it depends on,
  # is not listed; a library a macro shares with the program is still reached on its own path.
  # Build and dev dependencies, and workspace tools such as xtask, are not shipped either.
  def shipped:
    ($cargo[0].resolve.nodes | map({key: .id, value: .}) | from_entries) as $nodes
    | ([$cargo[0].packages[]
        | select(any(.targets[]; any(.kind[]; . == "proc-macro")))
        | {key: .id, value: true}] | from_entries) as $macros
    | {seen: {}, queue: [$cargo[0].packages[] | select(.name == "ma2a-app") | .id]}
    | until(.queue | length == 0;
        .queue[0] as $id
        | .queue |= .[1:]
        | if .seen[$id] then .
          else
            .seen[$id] = true
            | .queue += [$nodes[$id].deps[]
                | select(any(.dep_kinds[]; .kind == null) and ($macros[.pkg] | not))
                | .pkg]
          end)
    | .seen;
  def rust_packages:
    shipped as $shipped
    | [$cargo[0].packages[] | select($shipped[.id])]
    | to_entries
    | map({
        SPDXID: ("SPDXRef-Rust-" + (.key | tostring)),
        name: .value.name,
        versionInfo: .value.version,
        downloadLocation: (
          if .value.source == "registry+https://github.com/rust-lang/crates.io-index" then
            ("https://crates.io/crates/" + .value.name + "/" + .value.version)
          else
            "NOASSERTION"
          end
        ),
        filesAnalyzed: false,
        licenseConcluded: "NOASSERTION",
        licenseDeclared: (.value.license // "NOASSERTION" | gsub("\\s*/\\s*"; " OR ")),
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
