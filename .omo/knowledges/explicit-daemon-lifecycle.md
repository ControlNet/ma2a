# Explicit daemon lifecycle

Updated 2026-09-22.

- Public lifecycle commands are `start`, `restart`, and `stop`; `shutdown` is removed. The existing `daemon` foreground entry remains available for supervision.
- `start` is idempotent and waits for private IPC readiness. `restart` gracefully stops the current daemon, waits for the lifetime lock to be released, and launches a replacement. If stopped, restart launches it. `stop` requires a running daemon and waits for teardown.
- `daemon_control.rs` replaces `autostart.rs`. Only start/restart call the background spawn helper. Business commands, credential control, and UI lifecycle commands use `require`, which probes without preparing directories or creating runtime state.
- Absent-daemon errors instruct operators to start the daemon with the same state directory. Version mismatch and permission errors remain distinct. Stdin-based Echo and invitation input check daemon availability before waiting for input.
- The startup lock serializes lifecycle operations, including the entire restart. The daemon lifetime lock must be dropped before spawning the child; otherwise the child can see the parent's lock and wait for a daemon that never starts.
- Persistent identity and credentials survive restart. WebUI always starts stopped after daemon restart.

Verification command:

```sh
cargo test -p ma2a-app --test daemon_lifecycle --test cli --test credential_daemon --test cli_ui_workflow --test cli_workflows --test cli_json --test cli_revoke --test cli_invite_security --test cli_secrets
```

Expected: explicit start convergence, idempotency, restart identity preservation, rejection of old shutdown, and no implicit state creation all pass. An earlier run passed 42 tests and failed one startup in the lifecycle suite before the lock ordering fix. That failure did not capture stderr, so its exact cause was not established. The lock ordering fix has not been retested.

After the lock ordering and early stdin checks were applied, `cargo check -p ma2a-app --all-targets` passed. No further runtime tests were executed.

## Replacing the executable while a daemon is running

- IPC handshake validation checks only the local API version and result type, not the daemon package version or build identity. The daemon reports its package version, but the client does not enforce equality.
- With compatible IPC, business commands continue using the old daemon; `start` reuses it. Explicit `restart` stops it and spawns the executable used by the invoking CLI. New operations unsupported by the old daemon can still fail even when the handshake succeeds.
- Incompatible IPC still prevents business commands from proceeding. Explicit start/stop/restart now recover from `IpcError::VersionMismatch` using `LocalApiClient::shutdown_compatible`. This narrow method obtains the daemon's advertised version from a handshake or version_mismatch response and sends only graceful_shutdown in that version with a fresh request ID. It validates the version, request ID, and shutting_down acknowledgement. No generic cross-version business API is exposed.
- The startup lock covers compatible shutdown, lifetime-lock release, and replacement. Start/restart then spawn the invoking CLI executable; stop leaves the daemon stopped. No PID signalling or forced termination is used. Failed acknowledgement or teardown aborts replacement.
- Future API versions must preserve the IPC location and framing, the advertised version field, and the graceful_shutdown request/response contract. Arbitrary incompatible transport formats cannot be recovered through this mechanism.
- Handshake and normal shutdown encoding use `LOCAL_API_VERSION`, so bumping it does not leave those requests hard-coded to API v1.

Compatibility fallback verification: `cargo check -p ma2a-app --all-targets` passed. `cargo clippy -p ma2a-app -p ma2a-runtime --no-deps -- -A clippy::too_many_arguments` passed with match_same_arms warnings; the app warning was subsequently fixed, while the existing runtime snapshot warning remains. The Unix process-level lifecycle test covers start, restart, and stop against an API v2 mock daemon. It passed. The larger selected CLI suite later hit one unrelated transient `private IPC frame is invalid` failure in the relay workflow; that exact test passed immediately when rerun alone.
