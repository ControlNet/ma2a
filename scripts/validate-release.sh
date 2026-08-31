#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 2 ]]; then
  printf 'usage: %s ARCHIVE TARGET\n' "$0" >&2
  exit 2
fi

archive=$1
target=$2
checksum_file="${archive}.sha256"
temporary=$(mktemp -d)
trap 'rm -rf "$temporary"' EXIT
raw_entries="$temporary/raw-entries"
entries="$temporary/entries"
expected="$temporary/expected"

[[ -f "$archive" ]] || { printf 'archive not found: %s\n' "$archive" >&2; exit 1; }
[[ -f "$checksum_file" ]] || { printf 'checksum not found: %s\n' "$checksum_file" >&2; exit 1; }

case "$target" in
  *-pc-windows-msvc)
    [[ "$archive" == *.zip ]] || { printf 'Windows target requires .zip archive\n' >&2; exit 1; }
    binary=ma2a.exe
    unzip -Z1 "$archive" | sed '/\/$/d' > "$raw_entries"
    ;;
  *-apple-darwin|*-unknown-linux-gnu)
    [[ "$archive" == *.tar.xz ]] || { printf 'Unix target requires .tar.xz archive\n' >&2; exit 1; }
    binary=ma2a
    tar -tJf "$archive" | sed '/\/$/d' > "$raw_entries"
    ;;
  *)
    printf 'unknown release target: %s\n' "$target" >&2
    exit 2
    ;;
esac

prefix=$(sed -n '1s#/.*##p' "$raw_entries")
[[ -n "$prefix" ]] || { printf 'archive has no top-level directory\n' >&2; exit 1; }
sed -n "s#^${prefix}/##p" "$raw_entries" | sort > "$entries"
[[ "$(wc -l < "$entries")" -eq "$(wc -l < "$raw_entries")" ]] || {
  printf 'archive contains entries outside its single top-level directory\n' >&2
  exit 1
}

printf '%s\n' LICENSE-APACHE LICENSE-MIT NOTICE README.md "$binary" quickstart.md | sort > "$expected"
if ! cmp -s "$expected" "$entries"; then
  printf 'archive contents differ from the release contract\nexpected:\n' >&2
  sed 's/^/  /' "$expected" >&2
  printf 'observed:\n' >&2
  sed 's/^/  /' "$entries" >&2
  exit 1
fi

checksum_name=$(awk 'NF >= 2 {print $2}' "$checksum_file" | sed 's/^\*//')
[[ "$checksum_name" == "$(basename "$archive")" ]] || {
  printf 'checksum sidecar names %s instead of %s\n' "$checksum_name" "$(basename "$archive")" >&2
  exit 1
}

expected_digest=$(awk 'NF >= 2 {print $1; exit}' "$checksum_file")
if command -v sha256sum >/dev/null 2>&1; then
  observed_digest=$(sha256sum "$archive" | awk '{print $1}')
else
  observed_digest=$(shasum -a 256 "$archive" | awk '{print $1}')
fi
[[ "$observed_digest" == "$expected_digest" ]] || { printf 'SHA-256 mismatch\n' >&2; exit 1; }

printf 'validated %s for %s\n' "$archive" "$target"
