#!/usr/bin/env bash
# Checks that validate-release-metadata.sh refuses an SBOM or license report that does not
# describe the release archive exactly, or that names where it was built.
set -euo pipefail

if [[ $# -ne 3 ]]; then
  printf 'usage: %s ARCHIVE SBOM LICENSE_REPORT\n' "$0" >&2
  exit 2
fi

archive=$1
sbom=$2
report=$3
temporary=$(mktemp -d)
trap 'rm -rf "$temporary"' EXIT

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

# A report that names a build directory is refused, however the tools spelled the path.
for mutation in \
  'unix-url|.rust["ma2a-app 0.1.0 path+file:///tmp/build/crates/ma2a-app"] = {licenses: ["MIT"]}' \
  'windows-url|.rust["ma2a-app 0.1.0 path+file:///D:/a/build/crates/ma2a-app"] = {licenses: ["MIT"]}' \
  'windows-path|.frontend.MIT += [{name: "ky", paths: ["D:\\a\\build\\web\\node_modules\\ky"]}]' \
  'mixed-path|.frontend.MIT += [{name: "ky", paths: ["d:/a/build/web/node_modules/ky"]}]' \
  'unix-path|.frontend.MIT += [{name: "ky", paths: ["/tmp/build/web/node_modules/ky"]}]' \
  'inner-unix-path|.rust["ma2a-app 0.1.0 (/tmp/build/crates/ma2a-app)"] = {licenses: ["MIT"]}' \
  'inner-windows-path|.rust["ma2a-app 0.1.0 (D:\\a\\build\\crates\\ma2a-app)"] = {licenses: ["MIT"]}' \
  'drive-url|.rust["ma2a-app 0.1.0 path+file://C:/a/build/crates/ma2a-app"] = {licenses: ["MIT"]}' \
  'host-url|.rust["ma2a-app 0.1.0 path+file://localhost/tmp/build/crates/ma2a-app"] = {licenses: ["MIT"]}'
do
  label=${mutation%%|*}
  mutated="$temporary/$label.licenses.json"
  jq "${mutation#*|}" "$report" > "$mutated"
  if ./scripts/validate-release-metadata.sh "$archive" "$sbom" "$mutated" >/dev/null 2>&1; then
    printf 'license report with a %s build path unexpectedly passed validation\n' "$label" >&2
    exit 1
  fi
done

# The described package must resolve to exactly the application package.
application=$(jq -r '.packages[] | select(.name == "ma2a-app") | .SPDXID' "$sbom")
other=$(jq -r '.packages[] | select(.name == "serde") | .SPDXID' "$sbom")
for mutation in \
  'dangling|.documentDescribes = ["SPDXRef-Missing"]' \
  "other|.documentDescribes = [\"$other\"]" \
  "duplicate|.packages += [.packages[] | select(.SPDXID == \"$application\") | .name = \"copy\"]"
do
  label=${mutation%%|*}
  mutated="$temporary/$label.spdx.json"
  jq "${mutation#*|}" "$sbom" > "$mutated"
  if ./scripts/validate-release-metadata.sh "$archive" "$mutated" "$report" >/dev/null 2>&1; then
    printf 'SBOM with a %s described package unexpectedly passed validation\n' "$label" >&2
    exit 1
  fi
done

# An archive with the same name but other content does not match this SBOM.
rebuilt="$temporary/rebuilt/$(basename "$archive")"
mkdir -p "$(dirname "$rebuilt")"
cp "$archive" "$rebuilt"
printf 'rebuilt' >> "$rebuilt"
if ./scripts/validate-release-metadata.sh "$rebuilt" "$sbom" "$report" >/dev/null 2>&1; then
  printf 'SBOM of another build with the same name unexpectedly passed validation\n' >&2
  exit 1
fi

printf 'release metadata probes passed\n'
