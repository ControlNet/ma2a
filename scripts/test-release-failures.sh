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

hash_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

./scripts/test-runtime-references.sh >/dev/null

if ! grep -Fqx 'url=$("$binary" --state-dir "$state_dir" ui start --json | jq -er .url)' ./scripts/smoke-release.sh; then
  printf 'release smoke must use the canonical ui start command\n' >&2
  exit 1
fi
if grep -Fq 'ui web' ./scripts/smoke-release.sh || grep -Fq 'state_dir" web' ./scripts/smoke-release.sh; then
  printf 'release smoke still invokes the removed web command\n' >&2
  exit 1
fi

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
printf '%s *%s\n' "$(hash_file "$missing")" "$(basename "$missing")" > "${missing}.sha256"
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
printf '%s *%s\n' "$(hash_file "$external")" "$(basename "$external")" > "${external}.sha256"
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

tooled="$temporary/tooled.spdx.json"
jq '.packages += [{SPDXID: "SPDXRef-Tool", name: "xtask", versionInfo: "0.0.0"}]' "$sbom" > "$tooled"
if ./scripts/validate-release-metadata.sh "$archive" "$tooled" "$report" >/dev/null 2>&1; then
  printf 'SBOM listing a workspace tool unexpectedly passed validation\n' >&2
  exit 1
fi

macro="$temporary/macro.spdx.json"
jq '.packages += [{SPDXID: "SPDXRef-Macro", name: "serde_derive", versionInfo: "1.0.0"}]' \
  "$sbom" > "$macro"
if ./scripts/validate-release-metadata.sh "$archive" "$macro" "$report" >/dev/null 2>&1; then
  printf 'SBOM listing a procedural macro unexpectedly passed validation\n' >&2
  exit 1
fi

# An archive with the same name but other content does not match this SBOM.
rebuilt="$temporary/rebuilt/$(basename "$archive")"
mkdir -p "$(dirname "$rebuilt")"
cp "$archive" "$rebuilt"
printf 'rebuilt' >> "$rebuilt"
if ./scripts/validate-release-metadata.sh "$rebuilt" "$sbom" "$report" >/dev/null 2>&1; then
  printf 'SBOM of another build with the same name unexpectedly passed validation\n' >&2
  exit 1
fi

# Windows cargo-dist .zip archives place files directly at the archive root; verify flat layout
# passes while mixed, stray-dir, or incomplete .zip archives are still rejected.
if command -v python3 >/dev/null 2>&1 && command -v unzip >/dev/null 2>&1; then
  win_flat="$temporary/ma2a-app-x86_64-pc-windows-msvc.zip"
  win_mixed="$temporary/mixed-x86_64-pc-windows-msvc.zip"
  win_empty_dir="$temporary/empty-dir-x86_64-pc-windows-msvc.zip"
  win_incomplete="$temporary/incomplete-x86_64-pc-windows-msvc.zip"
  python3 - "$win_flat" "$win_mixed" "$win_empty_dir" "$win_incomplete" <<'PY'
import sys, zipfile
files = ("LICENSE-APACHE", "LICENSE-MIT", "NOTICE", "README.md", "ma2a.exe", "quickstart.md")
with zipfile.ZipFile(sys.argv[1], "w") as zf:
    for name in files:
        zf.writestr(name, "x")
with zipfile.ZipFile(sys.argv[2], "w") as zf:
    for name in files:
        zf.writestr(name, "x")
    zf.writestr("extra/stray.txt", "x")
with zipfile.ZipFile(sys.argv[3], "w") as zf:
    for name in files:
        zf.writestr(name, "x")
    zf.writestr("stray/", "")
with zipfile.ZipFile(sys.argv[4], "w") as zf:
    for name in files[:-1]:
        zf.writestr(name, "x")
PY
  for z in "$win_flat" "$win_mixed" "$win_empty_dir" "$win_incomplete"; do
    printf '%s *%s\n' "$(hash_file "$z")" "$(basename "$z")" > "${z}.sha256"
  done
  ./scripts/validate-release.sh "$win_flat" x86_64-pc-windows-msvc >/dev/null
  for bad in "$win_mixed" "$win_empty_dir" "$win_incomplete"; do
    if ./scripts/validate-release.sh "$bad" x86_64-pc-windows-msvc >/dev/null 2>&1; then
      printf 'invalid Windows zip unexpectedly passed validation: %s\n' "$bad" >&2
      exit 1
    fi
  done
fi

# Prefix matching must be literal, never a regex where '.' matches arbitrary characters.
mkdir -p "$temporary/dot-prefix/ma2a-0.1.0" "$temporary/dot-prefix/ma2a-0X1X0"
for name in LICENSE-APACHE LICENSE-MIT NOTICE README.md quickstart.md; do
  printf 'x' > "$temporary/dot-prefix/ma2a-0.1.0/$name"
done
printf 'x' > "$temporary/dot-prefix/ma2a-0X1X0/ma2a"
dot_bad="$temporary/dot-prefix.tar.xz"
tar -cJf "$dot_bad" -C "$temporary/dot-prefix" ma2a-0.1.0 ma2a-0X1X0
printf '%s *%s\n' "$(hash_file "$dot_bad")" "$(basename "$dot_bad")" > "${dot_bad}.sha256"
if ./scripts/validate-release.sh "$dot_bad" "$target" >/dev/null 2>&1; then
  printf 'multi-directory archive with dot-matching prefix unexpectedly passed validation\n' >&2
  exit 1
fi

printf 'release failure probes passed\n'
