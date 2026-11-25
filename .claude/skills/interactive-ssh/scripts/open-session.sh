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

# The new run is the first one after those that already exist, whatever the local clock says.
before=$(gh run list -R "$repo" --workflow interactive.yml --event workflow_dispatch --limit 20 \
  --json databaseId --jq '[.[].databaseId] | max // 0')
gh workflow run interactive.yml -R "$repo" -f os="$os" -f ref="$ref" -f timeout_minutes="$timeout"

run=''
for _ in $(seq 60); do
  sleep 5
  run=$(gh run list -R "$repo" --workflow interactive.yml --event workflow_dispatch --limit 20 \
    --json databaseId --jq "[.[].databaseId | select(. > $before)] | min // empty")
  [ -z "$run" ] || break
done
[ -n "$run" ] || { echo "the dispatched run did not appear within 5 minutes" >&2; exit 1; }
echo "run: https://github.com/$repo/actions/runs/$run"

[ -n "$out" ] || out=$(mktemp -d "${TMPDIR:-/tmp}/interactive-ssh-$run.XXXXXX")
mkdir -p "$out"
printf '%s\n' \
  '@cert-authority uptermd.upterm.dev ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAICiecex8Dq718eSe1CCLgLvDmI7AagvCtax7brPFWkh4' \
  '@cert-authority [uptermd.upterm.dev]:443 ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAICiecex8Dq718eSe1CCLgLvDmI7AagvCtax7brPFWkh4' \
  > "$out/known_hosts"
: > "$out/ssh_config"

failed=0
for runner in "${runners[@]}"; do
  # A runner whose job ends before it hands off is reported; the others are still handed off.
  until gh run download "$run" -R "$repo" -n "ssh-$runner" -D "$out/$runner" > /dev/null 2>&1; do
    status=$(gh run view "$run" -R "$repo" --json status,jobs --jq "if .status == \"completed\"
      then .status else .jobs[] | select(.name == \"session ($runner)\") | .status end")
    if [ "$status" = completed ]; then
      echo "$runner: its job ended before it handed off a session" >&2
      failed=1
      continue 2
    fi
    sleep 10
  done
  target=$(awk 'NR == 1 { print $2 }' "$out/$runner/ssh-command.txt")
  [[ "$target" =~ ^[^[:space:]@\"]+@[^[:space:]@\"]+$ ]] || {
    echo "$runner: unexpected connection command" >&2
    exit 1
  }
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
done
exit "$failed"
