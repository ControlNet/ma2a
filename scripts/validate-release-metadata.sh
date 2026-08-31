#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 3 ]]; then
  printf 'usage: %s ARCHIVE SBOM LICENSE_REPORT\n' "$0" >&2
  exit 2
fi

archive=$1
sbom=$2
report=$3

jq -e '
  .spdxVersion == "SPDX-2.3" and
  (.packages | any(.name == "ma2a-app")) and
  (.packages | any(.name == "react")) and
  (.packages | any(.name == "react-dom")) and
  (.packages | any(.name == "ky")) and
  (.packages | any(.name == "zod"))
' "$sbom" >/dev/null

jq -e '
  (.rust | type == "object") and
  (.frontend | type == "object") and
  (.frontend.MIT | any(.name == "react"))
' "$report" >/dev/null

[[ "$(jq -r '.name' "$sbom")" == "$(basename "$archive")" ]] || {
  printf 'SBOM name does not bind the archive filename\n' >&2
  exit 1
}

printf 'validated release metadata for %s\n' "$archive"
