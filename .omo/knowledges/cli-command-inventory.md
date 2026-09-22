# Public CLI inventory

Updated for the simplified Space CLI on 2026-09-22: `crates/ma2a-app/src/cli.rs`, `main.rs`, `output.rs`, `daemon_control.rs`, and `commands/workflows/{space,space_reference,space_invite,relay}.rs`.

- Global option: `--state-dir`; standard help/version flags are provided by Clap.
- Lifecycle: `start` (idempotent background launch), `restart` (stop then background launch, or launch if stopped), `stop` (requires running daemon), `daemon` (foreground), `status [--json]` (requires running daemon).
- Identity: `endpoint show [--json]`.
- Spaces: `space create <NAME> [--json]`, `space list [--json]`, `space show <SPACE_REF> [--json]`. The name is positional and becomes shared genesis metadata; `--name` is gone.
- `SPACE_REF` is positional everywhere a Space is identified. A complete 64-character lowercase-hex Space ID resolves exactly; otherwise the reference is matched exactly against shared Space names. Zero matches is not-found (exit 1), one match resolves, several matches fail as ambiguous (exit 2) and list the Space IDs. No fuzzy matching, and duplicate names are legal.
- Invitations: `space invite <SPACE_REF> [--ttl DURATION]` prints only the ticket to stdout; TTL supports ms/s/m from 1 ms to 5 minutes and defaults to 5m. `space invite create`, `--file`, and `--stdout` are removed. The Runtime still writes the ticket to an owner-only file under the state directory, which the CLI reads, prints, and deletes, so no invite secret enters a JSON envelope.
- Acceptance: `space accept` takes no flags. A TTY prompts `Invite:` without echo; otherwise the ticket is read from stdin. `space invite redeem`, `--stdin`, and `--file` are removed, and any argv value beginning `ma2ainvite` is rejected everywhere.
- Departure: `space leave <SPACE_REF> [--json]` asks the Space authority to sign the next manifest generation removing this Endpoint; an unreachable authority fails without local change, and the owner of a Space cannot leave it.
- Membership: `space member remove <SPACE_REF> <ENDPOINT_ID> [--json]`, still a cryptographic `space_revoke`. `space member revoke` is removed.
- Control sync: `space sync status --endpoint [--json]`, `space sync now --endpoint [--json]`; diagnostics only, scoped to a peer Endpoint.
- Private relay: `relay private configure --listen --public-url --serve-space` with repeatable `--serve-space`, plus either both `--tls-cert` and `--tls-key` or `--external-tls`. Also `relay private disable` and `relay private status [--json]`.
- Public relay fallback: `relay public configure --url`, `relay public disable`, `relay public status [--json]`.
- Echo: `echo --endpoint` with exactly one of `--text` or `--stdin`, optionally `--json`.
- UI: `ui init`, `ui revoke-all`, `ui start [--host HOST] [--port PORT] [--json]`, `ui stop`, `ui status [--json]`.
- `ui start` defaults to 127.0.0.1 and port 0, accepts arbitrary binding IPs/hostnames including wildcard addresses, restarts an existing UI, and returns after binding while HTTP runs in the daemon. No UI state survives daemon restart.
- Human output prints `Name:` and `Space ID:` for every Space result, and surfaces a Runtime error envelope as its real protocol error plus remediation instead of "missing result type". `--json` still prints the raw envelope, and an error envelope now also exits non-zero.
- All business and UI commands require a running daemon and never launch one. UI JSON output is the status payload (`running`, nullable `url`), not the general Runtime envelope.
- `daemon-detached` is hidden and used internally by explicit start/restart. Starting a daemon never starts WebUI.
- Removed public commands: `shutdown`, top-level `init`, `ui open`, `ui password set/reset`, `ui sessions revoke-all`, `space invite create`, `space invite redeem`, `space member revoke`.
