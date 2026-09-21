# Explicit WebUI lifecycle

Updated 2026-09-21 for manual WebUI control.

Daemon lifecycle update (2026-09-22): all UI commands now require an already running daemon. Run `ma2a --state-dir "$STATE" start` before `ui init` or other UI commands. Even `ui status/stop` report an error if the daemon is absent. No UI command starts a daemon.

- `crates/ma2a-app/src/daemon.rs` creates a stopped `WebLifecycle` and attaches it to private IPC. Starting a daemon or Endpoint never binds HTTP. Daemon shutdown stops WebUI before Runtime cleanup.
- `ui init` uses the new `ui_init` local API operation. Store `PasswordTransition::Init` sets or resets the verifier atomically, increments the auth epoch, and revokes all prior sessions. It does not start HTTP. Lower-level explicit Set/Reset transitions remain available to local API callers.
- `ui start` requires a password, accepts arbitrary binding IPs/hostnames, and starts or restarts a daemon-owned HTTP task. Defaults are 127.0.0.1 and OS-selected port 0. The user's explicit requirement is that `--host` is NOT restricted to loopback.
- `ui stop` closes listener and connection tasks, including SSE, without stopping daemon/Endpoint. `ui status` reports running/stopped and actual bound URL; JSON uses `running` and nullable `url`. Wildcard URLs describe binding; remote browsers use a reachable node address.
- `ui start/stop/status` are volatile instructions without request IDs or durable replay; every start executes. Lifecycle requests bypass the durable mutation mutex so HTTP requests using IPC cannot deadlock UI stop.
- Listener lifetime is owned through a join handle and cancellation token. Each accepted HTTP connection is owned by a JoinSet. Stop cancels serving, releases the listener, aborts connections, and drains tasks. Service failures leave the Endpoint running; status checks the service task.
- Same-port restarts stop the old service first when sockets overlap. Other binding changes prepare the new listener before stopping the old one, so a failed bind preserves a working UI where possible.
- Explicit bindings accept the configured hostname or resolved IP at the selected port; wildcard bindings allow any valid Host at that port. Mutations still require exact same-origin Origin and CSRF. Duplicate/cross-site Fetch Metadata is rejected; absent metadata is accepted for explicitly started HTTP listeners because browsers can omit it outside trustworthy URLs ([W3C](https://www.w3.org/TR/fetch-metadata/)).
- Credentials persist; running state, host, and port do not. A daemon restart always leaves UI stopped.
- Removed CLI commands: top-level init, ui password set/reset, ui sessions revoke-all, ui open. Browser-launching helper removed. No remote HTTP routes expose lifecycle or password initialization.

Validation commands (compile/type-check only; do not imply runtime tests were run):

```sh
cargo check -p ma2a-app --all-targets
cargo check -p ma2a-runtime --all-targets
cd web && bunx tsc --noEmit
```

Existing fixtures were adapted to the command names and schema; their passwords are synthetic test data, never operator credentials.

Results in this change:

- Both Cargo checks above completed successfully, including compilation of existing test targets.
- TypeScript `tsc --noEmit` completed successfully.
- `cargo clippy -p ma2a-app -p ma2a-runtime -p ma2a-store --no-deps -- -A clippy::too_many_arguments` completed successfully; only the pre-existing `match_same_arms` warning in `actor/snapshot/project.rs` remained. The allowance accounts for existing snapshot constructor argument-count violations.
- Existing tests were updated for renamed commands and changed schema fixtures, but no tests or runtime smoke scenarios were executed.
