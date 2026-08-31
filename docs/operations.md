# Operations

## Runtime Lifecycle

Use `ma2a status` to inspect or auto-start the current-user Runtime, `ma2a daemon` for foreground
supervision, and `ma2a shutdown` for graceful stop. Use `--state-dir ABSOLUTE_PATH` for isolated
instances. The default locations and private IPC protections are documented in
[private-ipc.md](private-ipc.md).

Set or reset the loopback Web password only through `ma2a init` or
`ma2a ui password set|reset`. Passwords are never accepted in argv or URLs. Automation may opt into
two newline-delimited values on inherited standard input by setting `MA2A_PASSWORD_STDIN=1`; keep
that pipe private and do not log it.

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

- `status` cannot start: verify the state directory is absolute, local, current-user owned, and not
  shared with another incompatible daemon.
- Web UI redirects to setup: run `ma2a init`; browser-based password setup is intentionally absent.
- Web UI cannot load: use the exact loopback URL returned by `ma2a web` or `ma2a ui open`; proxying
  or rewriting Host/Origin values fails closed.
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
access, browser login, authenticated snapshot access, logout revocation, and same-origin availability
of every runtime asset referenced by the embedded HTML, CSS, and JavaScript graph.
