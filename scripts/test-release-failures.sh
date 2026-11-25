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

# The sidecar is one record for the archive, as `sha256sum --check` reads it. A second record
# that disagrees fails that command, so it fails validation too; a lone record with Windows line
# endings passes.
resummed="$temporary/resummed/$(basename "$archive")"
mkdir -p "$(dirname "$resummed")"
cp "$archive" "$resummed"
digest=$(hash_file "$archive")
{ printf '%s *%s\n' "$digest" "$(basename "$archive")"
  printf '%s *%s\n' "$(printf '0%.0s' $(seq 64))" "$(basename "$archive")"; } > "${resummed}.sha256"
if ./scripts/validate-release.sh "$resummed" "$target" >/dev/null 2>&1; then
  printf 'checksum sidecar with a second record unexpectedly passed validation\n' >&2
  exit 1
fi
printf '%s *%s\r\n' "$digest" "$(basename "$archive")" > "${resummed}.sha256"
./scripts/validate-release.sh "$resummed" "$target" >/dev/null

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

./scripts/test-release-executable.sh "$archive" "$target"

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

# Browsers read a backslash as a slash, so these network references leave the origin too.
for spelling in '/\bad.test/xxxxxxxxxxxx' '\\bad.test/xxxxxxxxxxxx'; do
  mkdir "$temporary/backslash-extracted"
  tar -xJf "$archive" -C "$temporary/backslash-extracted"
  backslash_prefix=$(find "$temporary/backslash-extracted" -mindepth 1 -maxdepth 1 -type d -print -quit)
  backslash_binary="$backslash_prefix/ma2a"
  # The same length keeps the binary's layout; only the console's copy of the path is replaced.
  SPELLING=$spelling perl -0777 -pi -e \
    's{URL\(`/api/v1/web/auth/logout`}{URL(`$ENV{SPELLING}`}g or die "no logout reference\n"' \
    "$backslash_binary"
  backslash="$temporary/backslash-reference.tar.xz"
  tar -cJf "$backslash" -C "$temporary/backslash-extracted" "$(basename "$backslash_prefix")"
  printf '%s *%s\n' "$(hash_file "$backslash")" "$(basename "$backslash")" > "${backslash}.sha256"
  ./scripts/validate-release.sh "$backslash" "$target" >/dev/null
  if ./scripts/smoke-release.sh "$backslash" "$target" >/dev/null 2>&1; then
    printf 'archive with a network reference %s unexpectedly passed smoke\n' "$spelling" >&2
    exit 1
  fi
  rm -rf "$temporary/backslash-extracted" "$backslash" "${backslash}.sha256"
done

./scripts/test-release-metadata.sh "$archive" "$sbom" "$report" >/dev/null

# Windows cargo-dist .zip archives place files directly at the archive root; verify flat layout
# passes while mixed, stray-dir, or incomplete .zip archives are still rejected.
if command -v python3 >/dev/null 2>&1 && command -v unzip >/dev/null 2>&1; then
  win_flat="$temporary/ma2a-app-x86_64-pc-windows-msvc.zip"
  win_mixed="$temporary/mixed-x86_64-pc-windows-msvc.zip"
  win_empty_dir="$temporary/empty-dir-x86_64-pc-windows-msvc.zip"
  win_incomplete="$temporary/incomplete-x86_64-pc-windows-msvc.zip"
  win_rooted="$temporary/rooted-x86_64-pc-windows-msvc.zip"
  win_rooted_dir="$temporary/rooted-dir-x86_64-pc-windows-msvc.zip"
  win_link="$temporary/link-x86_64-pc-windows-msvc.zip"
  win_fifo="$temporary/fifo-x86_64-pc-windows-msvc.zip"
  win_dir_binary="$temporary/dir-binary-x86_64-pc-windows-msvc.zip"
  python3 - "$win_flat" "$win_mixed" "$win_empty_dir" "$win_incomplete" "$win_rooted" \
    "$win_rooted_dir" "$win_link" "$win_fifo" "$win_dir_binary" <<'PY'
import stat, sys, zipfile
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
for path, stray in ((sys.argv[5], False), (sys.argv[6], True)):
    with zipfile.ZipFile(path, "w") as zf:
        zf.writestr("ma2a/", "")
        for name in files:
            zf.writestr("ma2a/" + name, "x")
        if stray:
            zf.writestr("ma2a/stray/", "")
# The binary's entry is a link, a pipe, or a directory whose name has no trailing slash.
for path, kind in ((sys.argv[7], stat.S_IFLNK), (sys.argv[8], stat.S_IFIFO), (sys.argv[9], stat.S_IFDIR)):
    with zipfile.ZipFile(path, "w") as zf:
        for name in files:
            info = zipfile.ZipInfo("ma2a/" + name)
            if name == "ma2a.exe":
                info.create_system = 3
                info.external_attr = (kind | 0o755) << 16
            zf.writestr(info, "x")
PY
  for z in "$win_flat" "$win_mixed" "$win_empty_dir" "$win_incomplete" "$win_rooted" \
    "$win_rooted_dir" "$win_link" "$win_fifo" "$win_dir_binary"; do
    printf '%s *%s\n' "$(hash_file "$z")" "$(basename "$z")" > "${z}.sha256"
  done
  ./scripts/validate-release.sh "$win_flat" x86_64-pc-windows-msvc >/dev/null
  ./scripts/validate-release.sh "$win_rooted" x86_64-pc-windows-msvc >/dev/null
  for bad in "$win_mixed" "$win_empty_dir" "$win_incomplete" "$win_rooted_dir" "$win_link" \
    "$win_fifo" "$win_dir_binary"; do
    if ./scripts/validate-release.sh "$bad" x86_64-pc-windows-msvc >/dev/null 2>&1; then
      printf 'invalid Windows zip unexpectedly passed validation: %s\n' "$bad" >&2
      exit 1
    fi
  done
fi

# A top-level directory named like a drive would unpack outside the destination on Windows.
if command -v python3 >/dev/null 2>&1 && command -v unzip >/dev/null 2>&1; then
  drive="$temporary/drive-x86_64-pc-windows-msvc.zip"
  python3 - "$drive" <<'PY'
import sys, zipfile
files = ("LICENSE-APACHE", "LICENSE-MIT", "NOTICE", "README.md", "ma2a.exe", "quickstart.md")
with zipfile.ZipFile(sys.argv[1], "w") as zf:
    for name in files:
        zf.writestr(zipfile.ZipInfo("C:/" + name), "x")
PY
  printf '%s *%s\n' "$(hash_file "$drive")" "$(basename "$drive")" > "${drive}.sha256"
  if ./scripts/validate-release.sh "$drive" x86_64-pc-windows-msvc >/dev/null 2>&1; then
    printf 'zip under a drive-like top-level directory unexpectedly passed validation\n' >&2
    exit 1
  fi
fi

# A single top-level directory holds only the release files, not even an empty directory.
for layout in rooted rooted-dir; do
  mkdir -p "$temporary/$layout/ma2a-0.1.0"
  for name in LICENSE-APACHE LICENSE-MIT NOTICE README.md quickstart.md ma2a; do
    printf 'x' > "$temporary/$layout/ma2a-0.1.0/$name"
  done
  chmod +x "$temporary/$layout/ma2a-0.1.0/ma2a"
done
mkdir "$temporary/rooted-dir/ma2a-0.1.0/stray"
for layout in rooted rooted-dir; do
  tar -cJf "$temporary/$layout-x86_64-unknown-linux-gnu.tar.xz" -C "$temporary/$layout" ma2a-0.1.0
  archived="$temporary/$layout-x86_64-unknown-linux-gnu.tar.xz"
  printf '%s *%s\n' "$(hash_file "$archived")" "$(basename "$archived")" > "${archived}.sha256"
done
./scripts/validate-release.sh "$temporary/rooted-x86_64-unknown-linux-gnu.tar.xz" \
  x86_64-unknown-linux-gnu >/dev/null
if ./scripts/validate-release.sh "$temporary/rooted-dir-x86_64-unknown-linux-gnu.tar.xz" \
  x86_64-unknown-linux-gnu >/dev/null 2>&1; then
  printf 'archive with an extra directory under its top-level directory unexpectedly passed\n' >&2
  exit 1
fi

# A link in a .tar.xz could stand in for the binary, whether symbolic or hard.
for link in symbolic hard; do
  linked="$temporary/$link-link/ma2a-0.1.0"
  mkdir -p "$linked"
  for name in LICENSE-APACHE LICENSE-MIT NOTICE README.md quickstart.md; do
    printf 'x' > "$linked/$name"
  done
  if [[ "$link" == symbolic ]]; then ln -s README.md "$linked/ma2a"; else ln "$linked/README.md" "$linked/ma2a"; fi
  archived="$temporary/$link-link-x86_64-unknown-linux-gnu.tar.xz"
  tar -cJf "$archived" -C "$temporary/$link-link" ma2a-0.1.0
  printf '%s *%s\n' "$(hash_file "$archived")" "$(basename "$archived")" > "${archived}.sha256"
  if ./scripts/validate-release.sh "$archived" x86_64-unknown-linux-gnu >/dev/null 2>&1; then
    printf 'archive with a %s link unexpectedly passed validation\n' "$link" >&2
    exit 1
  fi
done

# tar unpacks an entry of a type it does not know as a plain file, but the entry is no plain file.
if command -v python3 >/dev/null 2>&1; then
  unknown="$temporary/unknown-type-x86_64-unknown-linux-gnu.tar.xz"
  python3 - "$unknown" <<'PY'
import io, sys, tarfile
names = ("LICENSE-APACHE", "LICENSE-MIT", "NOTICE", "README.md", "quickstart.md", "ma2a")
with tarfile.open(sys.argv[1], "w:xz") as tf:
    for name in names:
        info = tarfile.TarInfo("ma2a-0.1.0/" + name)
        info.size, info.mode = 1, 0o755
        if name == "ma2a":
            info.type = b"9"
        tf.addfile(info, io.BytesIO(b"x"))
PY
  printf '%s *%s\n' "$(hash_file "$unknown")" "$(basename "$unknown")" > "${unknown}.sha256"
  if ./scripts/validate-release.sh "$unknown" x86_64-unknown-linux-gnu >/dev/null 2>&1; then
    printf 'archive with an entry of unknown type unexpectedly passed validation\n' >&2
    exit 1
  fi
fi

# Prefix matching must be literal, never a regex where '.' matches arbitrary characters.
mkdir -p "$temporary/dot-prefix/ma2a-0.1.0" "$temporary/dot-prefix/ma2a-0X1X0"
for name in LICENSE-APACHE LICENSE-MIT NOTICE README.md quickstart.md; do
  printf 'x' > "$temporary/dot-prefix/ma2a-0.1.0/$name"
done
printf 'x' > "$temporary/dot-prefix/ma2a-0X1X0/ma2a"
chmod +x "$temporary/dot-prefix/ma2a-0X1X0/ma2a"
dot_bad="$temporary/dot-prefix.tar.xz"
tar -cJf "$dot_bad" -C "$temporary/dot-prefix" ma2a-0.1.0 ma2a-0X1X0
printf '%s *%s\n' "$(hash_file "$dot_bad")" "$(basename "$dot_bad")" > "${dot_bad}.sha256"
if ./scripts/validate-release.sh "$dot_bad" "$target" >/dev/null 2>&1; then
  printf 'multi-directory archive with dot-matching prefix unexpectedly passed validation\n' >&2
  exit 1
fi

printf 'release failure probes passed\n'
