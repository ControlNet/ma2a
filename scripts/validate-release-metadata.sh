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
  (.creationInfo.created | type == "string" and test("^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z$")) and
  (.documentDescribes | type == "array" and length == 1) and
  (.packages | all(.licenseDeclared | type == "string" and (contains("/") | not))) and
  (.packages | any(.name == "ma2a-app")) and
  (.packages | any(.name == "xtask") | not) and
  (.packages | any(.name == "serde")) and
  (.packages | any(.name == "serde_derive") | not) and
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

jq -e --arg expected "$(basename "$archive")" '.name == $expected' "$sbom" >/dev/null || {
  printf 'SBOM name does not bind the archive filename\n' >&2
  exit 1
}

# The SBOM names the archive by the digest of its content, not only by its file name, so an SBOM
# of another build with the same name is refused before it is attested.
if command -v sha256sum >/dev/null 2>&1; then
  digest=$(sha256sum "$archive" | awk '{print $1}')
else
  digest=$(shasum -a 256 "$archive" | awk '{print $1}')
fi
jq -e --arg expected "https://github.com/ControlNet/ma2a/releases/sha256/$digest" \
  '.documentNamespace == $expected' "$sbom" >/dev/null || {
  printf 'SBOM namespace does not bind the archive content\n' >&2
  exit 1
}

printf 'validated release metadata for %s\n' "$archive"
