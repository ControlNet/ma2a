#!/usr/bin/env bash
# Repacks a good Unix release archive with the program's execute bits cleared and expects both
# validation and the smoke test to refuse it.
set -euo pipefail

if [[ $# -ne 2 ]]; then
  printf 'usage: %s ARCHIVE TARGET\n' "$0" >&2
  exit 2
fi

archive=$1
target=$2
temporary=$(mktemp -d)
trap 'rm -rf "$temporary"' EXIT

mkdir "$temporary/extracted"
tar -xJf "$archive" -C "$temporary/extracted"
prefix=$(find "$temporary/extracted" -mindepth 1 -maxdepth 1 -type d -print -quit)
chmod a-x "$prefix/ma2a"
unexecutable="$temporary/unexecutable.tar.xz"
tar -cJf "$unexecutable" -C "$temporary/extracted" "$(basename "$prefix")"
if command -v sha256sum >/dev/null 2>&1; then
  digest=$(sha256sum "$unexecutable" | awk '{print $1}')
else
  digest=$(shasum -a 256 "$unexecutable" | awk '{print $1}')
fi
printf '%s *%s\n' "$digest" "$(basename "$unexecutable")" > "${unexecutable}.sha256"
if ./scripts/validate-release.sh "$unexecutable" "$target" >/dev/null 2>&1; then
  printf 'archive with a non-executable binary unexpectedly passed validation\n' >&2
  exit 1
fi
if ./scripts/smoke-release.sh "$unexecutable" "$target" >/dev/null 2>&1; then
  printf 'archive with a non-executable binary unexpectedly passed smoke\n' >&2
  exit 1
fi
