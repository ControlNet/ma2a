#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 2 ]]; then
  printf 'usage: %s ARCHIVE TARGET\n' "$0" >&2
  exit 2
fi

archive_directory=$(cd "$(dirname "$1")" && pwd)
archive="$archive_directory/$(basename "$1")"
target=$2
workspace=$(mktemp -d)
state_dir="$workspace/state"
binaries="$workspace/binaries"
cleanup() {
  if [[ -n "${binary:-}" && -x "${binary:-}" ]]; then
    "$binary" --state-dir "$state_dir" shutdown >/dev/null 2>&1 || true
  fi
  rm -rf "$workspace"
}
trap cleanup EXIT

mkdir -m 700 "$state_dir"
case "$target" in
  *-pc-windows-msvc)
    unzip -q "$archive" -d "$workspace/extracted"
    find "$workspace/extracted" -type f -name ma2a.exe -print > "$binaries"
    ;;
  *-apple-darwin|*-unknown-linux-gnu)
    mkdir "$workspace/extracted"
    tar -xJf "$archive" -C "$workspace/extracted"
    find "$workspace/extracted" -type f -name ma2a -print > "$binaries"
    ;;
  *)
    printf 'unknown release target: %s\n' "$target" >&2
    exit 2
    ;;
esac

binary_count=$(wc -l < "$binaries")
[[ "$binary_count" -eq 1 ]] || {
  printf 'expected exactly one extracted ma2a binary, observed %s\n' "$binary_count" >&2
  exit 1
}
binary=$(sed -n '1p' "$binaries")
chmod 700 "$binary"

version=$($binary --version)
[[ "$version" == ma2a\ * ]] || { printf 'unexpected version output: %s\n' "$version" >&2; exit 1; }

status=$($binary --state-dir "$state_dir" status --json)
jq -e '.result.payload.endpoint.online == true and .result.payload.spaces == []' <<<"$status" >/dev/null

smoke_credential="ma2a-release-smoke-password"
printf '%s\n%s\n' "$smoke_credential" "$smoke_credential" |
  MA2A_PASSWORD_STDIN=1 "$binary" --state-dir "$state_dir" ui password set >/dev/null

created=$($binary --state-dir "$state_dir" space create --name release-smoke --json)
jq -e '.result.payload.space_id | type == "string"' <<<"$created" >/dev/null

url=$($binary --state-dir "$state_dir" web)
[[ "$url" == http://127.0.0.1:* ]] || { printf 'unexpected Web URL: %s\n' "$url" >&2; exit 1; }
html=$(curl --fail --silent --show-error "$url/login")
asset=$(sed -n 's/.*src="\/\([^"]*assets\/[^"]*\.js\)".*/\1/p' <<<"$html" | sed -n '1p')
[[ -n "$asset" ]] || { printf 'embedded JavaScript asset reference missing\n' >&2; exit 1; }
curl --fail --silent --show-error "$url/$asset" > "$workspace/embedded.js"
[[ -s "$workspace/embedded.js" ]] || { printf 'embedded JavaScript asset is empty\n' >&2; exit 1; }

$binary --state-dir "$state_dir" shutdown >/dev/null
binary=
printf 'clean-room smoke passed for %s\n' "$target"
