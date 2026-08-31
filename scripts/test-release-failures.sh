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

mkdir "$temporary/external-extracted"
tar -xJf "$archive" -C "$temporary/external-extracted"
external_prefix=$(find "$temporary/external-extracted" -mindepth 1 -maxdepth 1 -type d -print -quit)
external_binary="$external_prefix/ma2a"
reference_count=$(perl -0777 -ne '$count = () = m{/favicon\.svg}g; print $count' "$external_binary")
[[ "$reference_count" -eq 1 ]] || {
  printf 'expected one embedded favicon reference, observed %s\n' "$reference_count" >&2
  exit 1
}
perl -0777 -pi -e 's{/favicon\.svg}{//bad.test/x}' "$external_binary"
external="$temporary/external-reference.tar.xz"
tar -cJf "$external" -C "$temporary/external-extracted" "$(basename "$external_prefix")"
printf '%s *%s\n' "$(sha256sum "$external" | awk '{print $1}')" "$(basename "$external")" > "${external}.sha256"
./scripts/validate-release.sh "$external" "$target" >/dev/null
if ./scripts/smoke-release.sh "$external" "$target" >/dev/null 2>&1; then
  printf 'archive with an external runtime asset reference unexpectedly passed smoke\n' >&2
  exit 1
fi

incomplete="$temporary/incomplete.spdx.json"
jq '.packages |= map(select(.name != "react"))' "$sbom" > "$incomplete"
if ./scripts/validate-release-metadata.sh "$archive" "$incomplete" "$report" >/dev/null 2>&1; then
  printf 'SBOM without frontend dependency unexpectedly passed validation\n' >&2
  exit 1
fi

printf 'release failure probes passed\n'
