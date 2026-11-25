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
all_entries="$temporary/all-entries"
entries="$temporary/entries"
types="$temporary/types"
expected="$temporary/expected"

[[ -f "$archive" ]] || { printf 'archive not found: %s\n' "$archive" >&2; exit 1; }
[[ -f "$checksum_file" ]] || { printf 'checksum not found: %s\n' "$checksum_file" >&2; exit 1; }

case "$target" in
  *-pc-windows-msvc)
    [[ "$archive" == *.zip ]] || { printf 'Windows target requires .zip archive\n' >&2; exit 1; }
    binary=ma2a.exe
    # A zip entry made on Unix with no file type bits lists as '?' and unpacks as a plain file.
    unknown_type='?'
    unzip -Z1 "$archive" | tr -d '\r' > "$all_entries"
    # The long listing has two header lines and one totals line around one line per entry.
    unzip -Z "$archive" | sed '1,2d;$d' | cut -c1 > "$types"
    ;;
  *-apple-darwin|*-unknown-linux-gnu)
    [[ "$archive" == *.tar.xz ]] || { printf 'Unix target requires .tar.xz archive\n' >&2; exit 1; }
    binary=ma2a
    # tar lists an entry of a type it does not know as '?' as well; only '-' is a file there.
    unknown_type='-'
    tar -tJf "$archive" | tr -d '\r' > "$all_entries"
    tar -tvJf "$archive" | cut -c1 > "$types"
    ;;
  *)
    printf 'unknown release target: %s\n' "$target" >&2
    exit 2
    ;;
esac

# Every entry is a plain file, or a directory whose name ends in '/': a link, a pipe, a device,
# or a directory named like a file could stand in for the binary. The listings are read from files
# to the end, so a command that fails is never hidden by a reader that stopped early.
if [[ $(wc -l < "$types") -ne $(wc -l < "$all_entries") ]] ||
  ! paste "$types" "$all_entries" | awk -F '\t' -v unknown="$unknown_type" '
    { name = substr($0, 3) }
    name == "" || !((($1 == "-" || $1 == unknown) && name !~ /\/$/) || ($1 == "d" && name ~ /\/$/)) {
      bad = 1
    }
    END { exit bad }'; then
  printf 'archive contains an entry that is neither a file nor a directory\n' >&2
  exit 1
fi

strip_single_prefix() {
  local prefix
  prefix=$(awk -F/ 'NF >= 2 { print $1; exit }' "$all_entries")
  # A drive-like name or a backslash would unpack outside the destination on Windows; refusing a
  # backslash also keeps awk -v below from reading the name as escapes.
  if [[ -z "$prefix" || "$prefix" == "." || "$prefix" == ".." || "$prefix" == *[:\\]* ]]; then
    printf 'archive has no valid single top-level directory\n' >&2
    exit 1
  fi
  if ! awk -v p="${prefix}/" 'index($0, p) != 1 { exit 1 }' "$all_entries"; then
    printf 'archive contains entries outside its single top-level directory\n' >&2
    exit 1
  fi
  # Only the top-level directory's own record is dropped; any other directory record stays, so
  # an extra directory, even an empty one, fails the comparison below.
  awk -v p="${prefix}/" '
    {
      rel = substr($0, length(p) + 1)
      if (rel != "") print rel
    }
  ' "$all_entries" | sort > "$entries"
}

case "$target" in
  *-pc-windows-msvc)
    if ! grep -q '/' "$all_entries"; then
      sort "$all_entries" > "$entries"
    else
      strip_single_prefix
    fi
    ;;
  *)
    strip_single_prefix
    ;;
esac

printf '%s\n' LICENSE-APACHE LICENSE-MIT NOTICE README.md "$binary" quickstart.md | sort > "$expected"
if ! cmp -s "$expected" "$entries"; then
  printf 'archive contents differ from the release contract\nexpected:\n' >&2
  sed 's/^/  /' "$expected" >&2
  printf 'observed:\n' >&2
  sed 's/^/  /' "$entries" >&2
  exit 1
fi

# The sidecar holds exactly one record, for this archive, in the form `sha256sum --check` reads.
# Blank lines, such as the one dist ends it with, are allowed, and so are Windows line endings,
# as a Windows build may write them and Windows users compare the digest by hand.
checksum_record=$(tr -d '\r' < "$checksum_file" | grep -v '^$' || true)
expected_digest=${checksum_record%% *}
[[ "$expected_digest" =~ ^[0-9a-f]{64}$ &&
  ( "$checksum_record" == "$expected_digest  $(basename "$archive")" ||
    "$checksum_record" == "$expected_digest *$(basename "$archive")" ) ]] || {
  printf 'checksum sidecar is not one record for %s\n' "$(basename "$archive")" >&2
  exit 1
}

if command -v sha256sum >/dev/null 2>&1; then
  observed_digest=$(sha256sum "$archive" | awk '{print $1}')
else
  observed_digest=$(shasum -a 256 "$archive" | awk '{print $1}')
fi
[[ "$observed_digest" == "$expected_digest" ]] || { printf 'SHA-256 mismatch\n' >&2; exit 1; }

# A Unix program has to be executable as an ordinary extraction leaves it.
case "$target" in
  *-pc-windows-msvc) ;;
  *)
    mkdir "$temporary/extracted"
    tar -xJf "$archive" -C "$temporary/extracted"
    extracted=$(find "$temporary/extracted" -type f -name "$binary" -print)
    [[ -x "$extracted" ]] || { printf 'archive binary %s is not executable\n' "$binary" >&2; exit 1; }
    ;;
esac

printf 'validated %s for %s\n' "$archive" "$target"
