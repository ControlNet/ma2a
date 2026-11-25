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

# Each excluded package is a copy of a valid one under another name, so only its exclusion can
# refuse it.
for mutation in \
  'workspace tool|xtask' \
  'procedural macro|serde_derive'
do
  label=${mutation%%|*}
  mutated="$temporary/${mutation#*|}.spdx.json"
  jq --arg name "${mutation#*|}" \
    '.packages += [.packages[] | select(.name == "serde") | .SPDXID = "SPDXRef-Copy" | .name = $name]' \
    "$sbom" > "$mutated"
  if ./scripts/validate-release-metadata.sh "$archive" "$mutated" "$report" >/dev/null 2>&1; then
    printf 'SBOM listing a %s unexpectedly passed validation\n' "$label" >&2
    exit 1
  fi
done

# Every package declares a license, as an SPDX expression.
for mutation in \
  'missing|.packages[0] |= del(.licenseDeclared)' \
  'slashed|.packages[0].licenseDeclared = "MIT / Apache-2.0"' \
  'dangling|.packages[0].licenseDeclared = "MIT OR "' \
  'empty|.packages[0].licenseDeclared = ""' \
  'file|.packages[0].licenseDeclared = "SEE LICENSE IN LICENSE.md"' \
  'unlicensed|.packages[0].licenseDeclared = "UNLICENSED"' \
  'unbalanced|.packages[0].licenseDeclared = "(MIT OR Apache-2.0"' \
  'and-with|.packages[0].licenseDeclared = "MIT AND WITH Classpath-exception-2.0"' \
  'or-with|.packages[0].licenseDeclared = "MIT OR WITH LLVM-exception"' \
  'parenthesized-with|.packages[0].licenseDeclared = "MIT AND (WITH LLVM-exception)"' \
  'exception-plus|.packages[0].licenseDeclared = "Apache-2.0 WITH LLVM-exception+"' \
  'none-plus|.packages[0].licenseDeclared = "NONE+"' \
  'unlicensed-plus|.packages[0].licenseDeclared = "UNLICENSED+"' \
  'and-plus|.packages[0].licenseDeclared = "AND+"' \
  'or-plus|.packages[0].licenseDeclared = "MIT AND OR+"' \
  'with-plus|.packages[0].licenseDeclared = "WITH+"' \
  'lowercase-ref|.packages[0].licenseDeclared = "licenseref-custom"' \
  'newline|.packages[0].licenseDeclared = "MIT\nOR Apache-2.0"' \
  'trailing-newline|.packages[0].licenseDeclared = "MIT\n"' \
  'punctuation|.packages[0].licenseDeclared = ". OR LICENSE"' \
  'unlisted|.packages[0].licenseDeclared = "MIT AND Foo-1.0"' \
  'license-as-exception|.packages[0].licenseDeclared = "Apache-2.0 WITH MIT"' \
  'exception-as-license|.packages[0].licenseDeclared = "LLVM-exception"' \
  'unlisted-exception|.packages[0].licenseDeclared = "Apache-2.0 WITH Foo-exception"' \
  'double-plus|.packages[0].licenseDeclared = "GPL-2.0++"'
do
  label=${mutation%%|*}
  mutated="$temporary/$label-license.spdx.json"
  jq "${mutation#*|}" "$sbom" > "$mutated"
  if ./scripts/validate-release-metadata.sh "$archive" "$mutated" "$report" >/dev/null 2>&1; then
    printf 'SBOM with a %s license unexpectedly passed validation\n' "$label" >&2
    exit 1
  fi
done

# A compound expression with parentheses and exceptions, one after a "+", is an SPDX expression,
# and so is one whose listed identifiers are written in another case.
compound="$temporary/compound-license.spdx.json"
for expression in \
  '(MIT OR Apache-2.0) AND Apache-2.0 WITH LLVM-exception OR GPL-2.0+ WITH Classpath-exception-2.0' \
  'mit OR APACHE-2.0 WITH llvm-exception'
do
  jq --arg expression "$expression" '.packages[0].licenseDeclared = $expression' "$sbom" > "$compound"
  ./scripts/validate-release-metadata.sh "$archive" "$compound" "$report" >/dev/null
done

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
  'host-url|.rust["ma2a-app 0.1.0 path+file://localhost/tmp/build/crates/ma2a-app"] = {licenses: ["MIT"]}' \
  'upper-url|.rust["ma2a-app 0.1.0 path+FILE:///tmp/build/crates/ma2a-app"] = {licenses: ["MIT"]}' \
  'climbing-url|.rust["ma2a-app 0.1.0 path+file://crates/../../tmp/build"] = {licenses: ["MIT"]}' \
  'climbing-fragment-url|.rust["ma2a-app 0.1.0 path+file://crates/..#0.1.0"] = {licenses: ["MIT"]}' \
  'climbing-parenthesized-url|.rust["ma2a-app 0.1.0 (path+file://crates/..)"] = {licenses: ["MIT"]}' \
  'climbing-backslash-url|.rust["ma2a-app 0.1.0 path+file://crates/..\\tmp\\build"] = {licenses: ["MIT"]}' \
  'climbing-spaced-url|.rust["ma2a-app 0.1.0 path+file://crates/a b/../../tmp/build"] = {licenses: ["MIT"]}' \
  'climbing-encoded-url|.rust["ma2a-app 0.1.0 path+file://crates/a/..%2f..%2ftmp"] = {licenses: ["MIT"]}' \
  'assigned-path|.rust["ma2a-app 0.1.0 path=/tmp/build/crates/ma2a-app"] = {licenses: ["MIT"]}' \
  'bracketed-path|.rust["ma2a-app 0.1.0 [D:\\a\\build\\crates\\ma2a-app]"] = {licenses: ["MIT"]}'
do
  label=${mutation%%|*}
  mutated="$temporary/$label.licenses.json"
  jq "${mutation#*|}" "$report" > "$mutated"
  if ./scripts/validate-release-metadata.sh "$archive" "$sbom" "$mutated" >/dev/null 2>&1; then
    printf 'license report with a %s build path unexpectedly passed validation\n' "$label" >&2
    exit 1
  fi
done

# A license expression written with a slash between spaces names no path.
spaced="$temporary/spaced.licenses.json"
jq '.frontend["MIT / Apache-2.0"] = [{name: "ky", versions: ["2.1.0"]}]' "$report" > "$spaced"
./scripts/validate-release-metadata.sh "$archive" "$sbom" "$spaced" >/dev/null

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
