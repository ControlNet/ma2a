#!/usr/bin/env bash
# Dispatch the interactive SSH workflow, wait for each runner's handoff artifact and write an ssh config for it.
# Usage: open-session.sh --repo OWNER/REPO --key PRIVATE_KEY [--os windows-latest] [--ref BRANCH] [--timeout 60] [--out DIR]
set -euo pipefail

repo='' key='' os='windows-latest' ref='' timeout=60 out=''
while [ $# -gt 0 ]; do
  case "$1" in
    --repo) repo=$2; shift 2 ;;
    --key) key=$2; shift 2 ;;
    --os) os=$2; shift 2 ;;
    --ref) ref=$2; shift 2 ;;
    --timeout) timeout=$2; shift 2 ;;
    --out) out=$2; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
[ -n "$repo" ] && [ -r "$key" ] || { echo "--repo and a readable --key are required" >&2; exit 2; }
# The handoff holds session tokens: nothing this script writes is readable by other users.
umask 077

case "$os" in
  all) runners=(ubuntu-latest windows-latest macos-15-intel) ;;
  *) runners=("$os") ;;
esac

# The workflow puts this dispatch's random request token in its run name, so the script finds its
# own run even when someone else dispatches one at the same time.
request=$(od -An -N8 -tx1 /dev/urandom | tr -d ' \n')
gh workflow run interactive.yml -R "$repo" -f os="$os" -f ref="$ref" -f timeout_minutes="$timeout" \
  -f request="$request"

run=''
for _ in $(seq 60); do
  sleep 5
  run=$(gh run list -R "$repo" --workflow interactive.yml --event workflow_dispatch --limit 50 \
    --json databaseId,displayTitle --jq ".[] | select(.displayTitle | endswith(\" $request\")) | .databaseId")
  [ -z "$run" ] || break
done
[ -n "$run" ] || { echo "the dispatched run did not appear within 5 minutes" >&2; exit 1; }
echo "run: https://github.com/$repo/actions/runs/$run"

[ -n "$out" ] || out=$(mktemp -d "${TMPDIR:-/tmp}/interactive-ssh-$run.XXXXXX")
mkdir -p "$out"
# The umask covers new files only, so an existing directory is made private and its files replaced.
chmod 700 "$out"
rm -f "$out/known_hosts" "$out/ssh_config"
printf '%s\n' \
  '@cert-authority uptermd.upterm.dev ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAICiecex8Dq718eSe1CCLgLvDmI7AagvCtax7brPFWkh4' \
  '@cert-authority [uptermd.upterm.dev]:443 ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAICiecex8Dq718eSe1CCLgLvDmI7AagvCtax7brPFWkh4' \
  > "$out/known_hosts"
: > "$out/ssh_config"

# Adds the ssh config entry for a runner whose handoff artifact was downloaded into a new
# directory, and only then puts that directory in place of the runner's, so a file left from an
# earlier run in a reused --out never stands in for one the artifact lacks.
hand_off() {
  local runner=$1 download=$2 target=''
  if [ -r "$download/ssh-command.txt" ] && [ -r "$download/workspace.txt" ]; then
    target=$(awk 'NR == 1 { print $2 }' "$download/ssh-command.txt")
  fi
  [[ "$target" =~ ^[^[:space:]@\"]+@[^[:space:]@\"]+$ ]] || {
    echo "$runner: unexpected connection command" >&2
    rm -rf "$download"
    return 1
  }
  rm -rf "${out:?}/$runner"
  mv "$download" "$out/$runner"
  cat >> "$out/ssh_config" <<EOF
Host $runner
  HostName ${target#*@}
  User ${target%@*}
  IdentityFile "$key"
  IdentitiesOnly yes
  IdentityAgent none
  BatchMode yes
  UserKnownHostsFile "$out/known_hosts"
  StrictHostKeyChecking yes
  ServerAliveInterval 30

EOF
  echo "$runner: ssh -F '$out/ssh_config' $runner   (workspace: $(cat "$out/$runner/workspace.txt"))"
}

# Every pending runner is checked on each round, so a session is handed off as soon as it is
# ready, whatever the other runners do. A runner whose job ends before it hands off, or whose
# handoff is malformed, is reported and fails the script; the others are still handed off.
failed=0
pending=("${runners[@]}")
while [ "${#pending[@]}" -gt 0 ]; do
  waiting=()
  for runner in "${pending[@]}"; do
    # The status is read before the download, so a job that uploads and then ends in between is
    # not mistaken for one that never handed off.
    status=$(gh run view "$run" -R "$repo" --json status,jobs --jq "if .status == \"completed\"
      then .status else .jobs[] | select(.name == \"session ($runner)\") | .status end")
    download=$(mktemp -d "$out/.$runner.XXXXXX")
    if gh run download "$run" -R "$repo" -n "ssh-$runner" -D "$download" > /dev/null 2>&1; then
      hand_off "$runner" "$download" || failed=1
    else
      rm -rf "$download"
      if [ "$status" = completed ]; then
        echo "$runner: its job ended before it handed off a session" >&2
        failed=1
      else
        waiting+=("$runner")
      fi
    fi
  done
  pending=(${waiting[@]+"${waiting[@]}"})
  [ "${#pending[@]}" -eq 0 ] || sleep 10
done
exit "$failed"
