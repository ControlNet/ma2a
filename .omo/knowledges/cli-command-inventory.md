# Public CLI inventory

Updated for explicit WebUI lifecycle on 2026-09-21: `crates/ma2a-app/src/cli.rs`, `main.rs`, and `commands/workflows/{space,relay}.rs`.

- Global option: `--state-dir`; standard help/version flags are provided by Clap.
- Lifecycle: `daemon` (foreground), `status [--json]` (runtime snapshot), `shutdown` (existing daemon only, no autostart).
- Identity: `endpoint show [--json]`.
- Spaces: `space create --name`, `space list`, `space show --space`; each supports `--json`. Show still returns the small Space summary. The newer `space_details_fetch` and `snapshot_stamp` are API operations, not standalone CLI subcommands.
- Invitations: `space invite create --space --ttl` requires either `--file` or interactive-terminal `--stdout`. TTL supports ms/s/m and is limited to 1 ms through 5 minutes. `space invite redeem` requires exactly one of `--stdin` or `--file`; invitation contents are rejected in argv.
- Membership: `space member revoke --space --endpoint [--json]`.
- Control sync: `space sync status --endpoint [--json]`, `space sync now --endpoint [--json]`; both are scoped to a peer Endpoint rather than a Space selector.
- Private relay: `relay private configure --listen --public-url --serve-space` with repeatable `--serve-space`, plus either both `--tls-cert` and `--tls-key` or `--external-tls`. Also `relay private disable` and `relay private status [--json]`.
- Public relay fallback: `relay public configure --url`, `relay public disable`, `relay public status [--json]`.
- Echo: `echo --endpoint` with exactly one of `--text` or `--stdin`, optionally `--json`.
- UI: `ui init` (atomic set/reset with session revocation), `ui revoke-all`, `ui start [--host HOST] [--port PORT] [--json]`, `ui stop`, `ui status [--json]`. Password input uses hidden confirmation prompts and accepts 1–1024 UTF-8 bytes.
- `ui start` defaults to 127.0.0.1 and port 0, accepts arbitrary binding IPs/hostnames including wildcard addresses, restarts an existing UI, and returns after binding while HTTP runs in the daemon. No UI state survives daemon restart.
- `ui stop/status` do not auto-start a daemon. UI JSON output is the status payload (`running`, nullable `url`), not the general Runtime envelope.
- `daemon-detached` is hidden and used for internal autostart. Starting a daemon never starts WebUI.
- Removed public commands: top-level `init`, `ui open`, `ui password set/reset`, `ui sessions revoke-all`.
