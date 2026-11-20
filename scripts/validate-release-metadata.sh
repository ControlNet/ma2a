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

[[ "$(jq -r '.name' "$sbom")" == "$(basename "$archive")" ]] || {
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
[[ "$(jq -r '.documentNamespace' "$sbom")" == "https://github.com/ControlNet/ma2a/releases/sha256/$digest" ]] || {
  printf 'SBOM namespace does not bind the archive content\n' >&2
  exit 1
}

printf 'validated release metadata for %s\n' "$archive"
