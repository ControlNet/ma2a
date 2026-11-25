#!/usr/bin/env bash
# Runs the license report generator from checkouts whose paths differ, and checks that the
# report never names the checkout and is the same wherever it was built.
set -euo pipefail

repo=$(git rev-parse --show-toplevel)
cd "$repo"
temporary=$(mktemp -d)
checkout="$temporary/spaced checkout"
# A checkout whose path ends with the whole path of a symlink to it.
nested="$temporary/outer$temporary/suffix"
cleanup() {
  git worktree remove --force "$checkout" 2>/dev/null || true
  git worktree remove --force "$nested" 2>/dev/null || true
  rm -rf "$temporary"
}
trap cleanup EXIT

./scripts/generate-license-report.sh "$temporary/plain.json"

# A logical path through a symlink differs from the physical path the tools print.
ln -s "$repo" "$temporary/linked checkout"
(cd "$temporary/linked checkout" && ./scripts/generate-license-report.sh "$temporary/linked.json")

# A physical path with a space is percent-encoded in Cargo's file URLs.
git worktree add --quiet --detach "$checkout" HEAD
(cd "$checkout/web" && bun install --frozen-lockfile --silent)
(cd "$checkout" && "$repo/scripts/generate-license-report.sh" "$temporary/spaced.json")

# A logical path that is a suffix of the physical one must not be cut out of the middle of it.
git worktree add --quiet --detach "$nested" HEAD
(cd "$nested/web" && bun install --frozen-lockfile --silent)
ln -s "$nested" "$temporary/suffix"
(cd "$temporary/suffix" && "$repo/scripts/generate-license-report.sh" "$temporary/suffix.json")

for report in linked spaced suffix; do
  if grep -qF -e "$temporary" -e "${temporary// /%20}" -e "$repo" "$temporary/$report.json"; then
    printf 'the %s report names its checkout\n' "$report" >&2
    exit 1
  fi
  if ! cmp -s <(jq -S . "$temporary/plain.json") <(jq -S . "$temporary/$report.json"); then
    printf 'the %s report differs from the plain one\n' "$report" >&2
    exit 1
  fi
done
printf 'license report is independent of the checkout path\n'
