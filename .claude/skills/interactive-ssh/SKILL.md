---
name: interactive-ssh
description: Open an SSH session on a GitHub-hosted Windows, macOS or Linux runner through the interactive workflow, then build, run and debug on that platform. Use when a platform-only failure cannot be reproduced locally, or when Windows or macOS behaviour needs several tries without pushing a full CI run each time.
---

# SSH into a CI runner

Debug locally whatever reproduces locally; a runner is for platform-only behaviour. Reproduce and
locate a platform failure in one session, then fix the code, instead of pushing CI runs by trial.

## Prerequisites

- `.github/workflows/interactive.yml` is on the default branch (`master`); a dispatch always reads
  the workflow from there and checks out the requested `ref`.
- The ref to test is pushed to `ControlNet/ma2a`.
- The connecting key's public half is in the repository variable `INTERACTIVE_SSH_AUTHORIZED_KEYS`
  (one key per line) or on the dispatching user's GitHub profile. Agents use a dedicated key
  registered in the variable, never a key added to the user's account (an account key can push to
  every repository of that account). Its private half stays outside the repository, at
  `~/.ssh/ma2a-interactive` by convention, with a comment that names no user or host.
- `gh` is logged in with Actions write access.

## Open a session

```bash
bash .claude/skills/interactive-ssh/scripts/open-session.sh \
  --repo ControlNet/ma2a --key ~/.ssh/ma2a-interactive --os windows-latest --ref <branch-or-sha> --timeout 60
```

- `--os` is `windows-latest`, `ubuntu-latest`, `macos-15-intel` or `all`; start only what you need.
- `--timeout` caps the whole job in minutes (at most 360).
- The script dispatches the workflow, waits for each runner's `ssh-<os>` artifact, writes an ssh
  config and prints `ssh -F <config> <os>` plus the workspace path. Expect about 2 minutes (the
  runner installs the Rust toolchain, Bun and the web dependencies first).

## Run commands

- `ssh -F <config> <os> '<command>'` runs one command; `scp -F <config>` copies files.
- On Windows the command runs in PowerShell; wrap bash work as `"bash -lc '<command>'"` and use
  `/d/a/...` paths.
- `cd` into the workspace from `<out>/<os>/workspace.txt` first. That path is POSIX (`/d/a/...`): use
  it inside `bash -lc`; PowerShell (pwsh 7 over ssh) wants `D:\a\...` and already starts there.
- Redirect long output to a log file on the runner and read it in pieces.
- Anything run through Git Bash (`bash -lc`, or a CI step with `shell: bash`) has
  SeBackupPrivilege and SeRestorePrivilege enabled, which a PowerShell CI step does not; with them,
  Windows skips file access checks. The session itself is started from a Git Bash step, so
  PowerShell in the session has them too. Check with `C:\Windows\System32\whoami.exe /priv`. Run ACL and permission tests through
  `scripts/drop-privileges.ps1`: copy it with
  `scp -F <config> .claude/skills/interactive-ssh/scripts/drop-privileges.ps1 windows-latest:D:/a/`, then
  `ssh -F <config> windows-latest 'cd D:\a\ma2a\ma2a -ErrorAction Stop; D:\a\drop-privileges.ps1 -Command "cargo test ..."; exit $LASTEXITCODE'`,
  or a test can pass in one shell and fail in the other. PowerShell needs the Windows form of the
  workspace path (`/d/a/ma2a/ma2a` is `D:\a\ma2a\ma2a`); `-ErrorAction Stop` ends the line if
  that path is wrong. The script exits with the command's exit code, or with 1 for a PowerShell
  error such as a command that does not exist or a failed cmdlet, as a `shell: pwsh` CI step does.
  The trailing `exit $LASTEXITCODE` hands that code to ssh; without it a failure ends as 1.
- Known session-only failures on master (they pass in CI): `backup::a_destination_name_near_the_component_limit_is_backed_up`
  and four `migration` tests in ma2a-store.
- Build and test as CI does, for example
  `cargo test --locked -p ma2a-store --lib windows_acl` or the Windows steps in
  `.github/workflows/ci.yml`.

## Sync code

- Commit and push a branch, then in the session `git fetch origin <branch> && git checkout --detach FETCH_HEAD`
  and rebuild incrementally.
- `scp` is fine for a one-file experiment, but a confirmed fix is committed locally; changes on the
  runner are lost with the session. A session result does not replace a normal CI run.

## End the session

- As soon as you are done: `ssh -F <config> <os> "bash -lc 'touch <workspace>/continue'"`, or
  `gh run cancel <run> -R ControlNet/ma2a`. Never leave a runner idle until its timeout. The
  workflow then stops the session, closing every shell, before its post steps run.

## Privacy and safety

- The connection command contains a session token: never put it in an issue, pull request, commit
  or any public text. Keep the generated config and downloaded artifacts in a temporary directory
  and delete them afterwards.
- Never print repository secrets, tokens or private keys in a session. Do not widen the authorized
  key list or open the session to anyone.
