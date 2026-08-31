#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 4 ]]; then
  printf 'usage: %s ARCHIVE TARGET SBOM LICENSE_REPORT\n' "$0" >&2
  exit 2
fi

archive=$1
target=$2
sbom=$3
report=$4
temporary=$(mktemp -d)
trap 'rm -rf "$temporary"' EXIT

tampered="$temporary/$(basename "$archive")"
cp "$archive" "$tampered"
cp "${archive}.sha256" "${tampered}.sha256"
printf 'tampered' >> "$tampered"
if ./scripts/validate-release.sh "$tampered" "$target" >/dev/null 2>&1; then
  printf 'tampered archive unexpectedly passed validation\n' >&2
  exit 1
fi

mkdir "$temporary/extracted"
tar -xJf "$archive" -C "$temporary/extracted"
prefix=$(find "$temporary/extracted" -mindepth 1 -maxdepth 1 -type d -print -quit)
rm "$prefix/quickstart.md"
missing="$temporary/missing-asset.tar.xz"
tar -cJf "$missing" -C "$temporary/extracted" "$(basename "$prefix")"
printf '%s *%s\n' "$(sha256sum "$missing" | awk '{print $1}')" "$(basename "$missing")" > "${missing}.sha256"
if ./scripts/validate-release.sh "$missing" "$target" >/dev/null 2>&1; then
  printf 'archive without quickstart unexpectedly passed validation\n' >&2
  exit 1
fi

incomplete="$temporary/incomplete.spdx.json"
jq '.packages |= map(select(.name != "react"))' "$sbom" > "$incomplete"
if ./scripts/validate-release-metadata.sh "$archive" "$incomplete" "$report" >/dev/null 2>&1; then
  printf 'SBOM without frontend dependency unexpectedly passed validation\n' >&2
  exit 1
fi

printf 'release failure probes passed\n'
