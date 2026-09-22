# Operations

## Runtime Lifecycle

Use `ma2a start` to start the current-user Runtime in the background, `ma2a restart` to restart it,
`ma2a stop` for graceful stop, and `ma2a daemon` for foreground supervision.
`ma2a status` only inspects a running Runtime. No business or UI command starts a daemon implicitly.
Use `--state-dir ABSOLUTE_PATH` for isolated
instances. The default locations and private IPC protections are documented in
[private-ipc.md](private-ipc.md).

`start` returns only once the daemon it launched has identified itself, and `stop` returns only once
that daemon has released the state directory. A `start` that fails has already terminated and reaped
the process it launched, so a failed start never leaves a daemon behind.

If a daemon still owns a state directory but has stopped answering, every lifecycle command fails
closed and says so, naming the process where the platform can report it:

```text
a daemon still owns this state directory but is not answering (…); it was not replaced.
Stop that process, then run `ma2a start`
```

MA2A will not start a second daemon beside it, unlink its endpoint, or repair ownership it cannot
account for. End that process yourself — `ma2a` writes the owning process identifier to
`run-v1/daemon.json` while it runs — and then start again. The same applies when ownership and the
responding daemon disagree, which is reported as ambiguous rather than silently resolved.

Never delete a state directory to recover from this. Removing `run-v1` unlinks the socket and the
lock of a process that is still running, which leaves a daemon nothing can address.

After replacing the executable, explicitly switch the running daemon to it with:

```sh
ma2a --state-dir "$STATE" restart
```

When the existing daemon's API version is incompatible, `start/restart` automatically stop it
using its reported protocol version and launch the current executable. `stop` can also stop an
incompatible daemon, but does not launch a replacement. These operations wait for graceful
teardown; a failed stop leaves replacement aborted. A compatible older daemon remains in use
until explicitly restarted. WebUI remains stopped after replacement.

Set or reset the Web password through `ma2a ui init`. Passwords are never accepted in argv or URLs. Automation may opt into
two newline-delimited values on inherited standard input by setting `MA2A_PASSWORD_STDIN=1`; keep
that pipe private and do not log it.

WebUI never starts automatically with the daemon, including after daemon restart. `ui init` only
updates credentials in the running daemon. Run `ma2a start` first, then explicitly run
`ma2a ui start` to serve HTTP in the background. Set a binding
with `--host` (IP or hostname, including wildcard addresses) and `--port`; defaults are `127.0.0.1`
and OS-selected port 0. A repeated start restarts WebUI. Use `ma2a ui status` for its running state
and URL, `ma2a ui stop` to close only HTTP/SSE connections, and `ma2a ui revoke-all` to invalidate
browser sessions. All UI commands fail when the daemon is stopped. Endpoint identity and networking survive UI
restarts.

## Enrollment and Synchronization

Create invitation tickets as owner-only files and transfer them through an authenticated private
channel. Tickets are short-lived bearer secrets. After redemption, use
`ma2a space sync status --endpoint ENDPOINT_ID` and `ma2a space sync now --endpoint ENDPOINT_ID` to
inspect or request bounded control synchronization. Revocation is Space-local and must name both the
Space and Endpoint.

## Relays and Degraded Mode

Private Relay native TLS requires an operator-provided valid certificate chain and owner-protected
private key. External TLS mode is allowed only with a loopback plaintext backend and an
operator-managed HTTPS proxy. Public fallback is disabled until explicitly configured.

MA2A reports configured relays, compatible candidates, and Iroh-observed effective home/path as
separate facts. `DegradedNoCommonHome` means active Spaces have no common compatible private relay
and no configured public fallback. It is not a claim that direct paths are impossible, and a listed
candidate is not a reachability guarantee. See [relay.md](relay.md) and
[reachability.md](reachability.md).

## Storage, Key Loss, and Backup

Do not copy a live SQLite file. The repository backup API uses SQLite online backup and produces a
checked database copy, but the CLI does not yet expose a complete backup/export workflow. The
database contains opaque key references; protected Endpoint and Space-authority key files require a
separate explicit secure export. Losing an Endpoint key loses that persistent Endpoint identity.
Losing a Space authority key prevents future authority-signed Space changes. Phase 1 has no recovery
wizard, cloud escrow, or automatic key replication.

## Troubleshooting

- A command reports that the daemon is not running: use `ma2a start` with the same `--state-dir`.
- `start` fails: verify the state directory is absolute, local, current-user owned, and not
  shared with another incompatible daemon.
- Web UI start requires a password: run `ma2a ui init`, then `ma2a ui start`.
- Web UI cannot load: use the URL returned by `ma2a ui status` (replace a wildcard address with the node address); Host and Origin must satisfy the selected binding and same-origin checks.
- Enrollment fails: confirm the ticket is unexpired, unused, intact, and intended for this Endpoint.
- Echo fails after revocation: this is expected when no complete shared Space still authorizes it.
- Private Relay is offline: verify TLS mode, certificate validity, key permissions, served-Space
  membership, provider capability, and public HTTPS reachability.
- Relay state is degraded: inspect compatible candidates and Iroh observations separately; do not
  infer a selected home from desired configuration.

## Release Verification

From a source checkout run:

```sh
cargo run --locked -p xtask -- check-release
cargo run --locked -p xtask -- check-support
bunx @axodotdev/dist@0.32.0 plan --output-format=json --allow-dirty
./scripts/validate-release.sh target/distrib/ARCHIVE TARGET
./scripts/smoke-release.sh target/distrib/ARCHIVE TARGET
```

For downloaded assets, verify SHA-256 first and then run `gh attestation verify` as shown in
[quickstart.md](quickstart.md). The SPDX SBOM must contain both Rust and production frontend
packages, and the dependency/license report must have both `rust` and `frontend` sections. The
clean-room smoke logs only HTTP methods, paths, and statuses while proving failed unauthenticated
access, browser login, authenticated snapshot access, server-side revocation of the original session
after logout, same-origin availability of literal HTML/CSS/module assets, and rejection of external
literal browser network sinks.
