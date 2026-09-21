# Endpoint-Centric CLI

MA2A administration targets Endpoint IDs. Commands that require Runtime state use the private
current-user daemon and fail if it is not running. Use `--state-dir PATH` to isolate a Runtime; otherwise
MA2A uses the platform state directory described in [private-ipc.md](private-ipc.md).

## Runtime and Endpoint

```sh
ma2a start
ma2a restart
ma2a stop
ma2a status [--json]
ma2a endpoint show [--json]
ma2a daemon
```

`start` explicitly starts the daemon in the background, waits for readiness, and returns to the
shell. An already running, protocol-compatible daemon is reused without restarting it. `restart` gracefully stops the
running daemon and starts it again; if stopped, it starts it. `stop` waits for graceful teardown
and reports an error if no daemon is running. These commands use the same `--state-dir` as the
business commands. The old `shutdown` command is removed.

If the daemon's API version is incompatible, `start` and `restart` automatically stop it through
the lifecycle compatibility path and launch the current CLI executable. `stop` uses the same
compatibility path to stop the old daemon and leaves it stopped. Business and UI commands never
perform this replacement. A different package version alone does not trigger replacement;
use `restart` to switch a compatible daemon to the updated executable.

`daemon` runs in the foreground for direct supervision. WebUI is stopped by default, including
after `restart`. No business or UI command implicitly starts the daemon. `status` reads the Runtime
snapshot and reports an error if the daemon is stopped; it never changes the running state.

## Spaces and Enrollment

```sh
ma2a space create --name LABEL [--json]
ma2a space list [--json]
ma2a space show --space SPACE_ID [--json]
ma2a space invite create --space SPACE_ID --ttl 30s --file OWNER_ONLY_PATH
ma2a space invite create --space SPACE_ID --ttl 5m --stdout
ma2a space invite redeem --file OWNER_ONLY_PATH
ma2a space invite redeem --stdin
ma2a space member revoke --space SPACE_ID --endpoint ENDPOINT_ID [--json]
ma2a space sync status --endpoint ENDPOINT_ID [--json]
ma2a space sync now --endpoint ENDPOINT_ID [--json]
```

Invite TTL accepts `ms`, `s`, or `m` and must be between `1ms` and `5m`. Creation writes the ticket
once using exclusive file creation and owner-only permissions. `--stdout` is accepted only on an
interactive terminal; terminal history and capture tools may retain displayed output. Redemption
accepts only a file or standard input. Invite tickets are never accepted as positional or option
values and never appear in JSON responses.

Revocation requires both the Space and Endpoint IDs. Synchronization targets the peer Endpoint;
the Runtime derives shared Spaces internally.

## Relays

```sh
ma2a relay private configure \
  --listen 127.0.0.1:443 \
  --public-url https://relay.example \
  --serve-space SPACE_ID \
  --tls-cert /secure/relay-cert.pem \
  --tls-key /secure/relay-key.pem

ma2a relay private configure \
  --listen 127.0.0.1:8080 \
  --public-url https://relay.example \
  --serve-space SPACE_ID \
  --external-tls

ma2a relay private status [--json]
ma2a relay private disable
ma2a relay public configure --url https://public-relay.example
ma2a relay public status [--json]
ma2a relay public disable
```

Private Relay configuration requires exactly one TLS mode: native TLS with both certificate and
private-key paths, or explicit external termination. Repeat `--serve-space` for each served Space.
Key contents never enter argv, local API JSON, output, or logs; only the owner-controlled path is
sent to the daemon. Public fallback is enabled only by the explicit configure command.

Relay status distinguishes configured state from observed online state. Runtime snapshots also
report compatible relay candidates separately from Iroh-observed effective relay and reachability
state; MA2A does not claim to select Iroh's preferred or home relay.

## Echo and Web UI

```sh
ma2a echo --endpoint ENDPOINT_ID --text TEXT [--json]
printf '%s' TEXT | ma2a echo --endpoint ENDPOINT_ID --stdin [--json]
ma2a ui init
ma2a ui revoke-all
ma2a ui start --host 127.0.0.1 --port 8080
ma2a ui stop
ma2a ui status --json
```

Echo is Endpoint-addressed and has no Space routing option. `ui init` reads and confirms a
1–1024-byte UTF-8 password without terminal echo. It sets the first password or resets an existing
one atomically, revoking all existing sessions. Passwords are rejected from argv.

`ui start` requires a running daemon and a configured password. It binds the WebUI
listener, prints its status and URL, and returns to the shell while the daemon serves HTTP in the
background. Running it again restarts WebUI. `--host` accepts IP addresses and hostnames, including
`0.0.0.0` and `::` for wildcard binding. The defaults are `127.0.0.1` and `--port 0` (a free port
selected by the OS). `--json` returns the same status shape as `ui status --json`.

`ui stop` closes the WebUI listener and existing HTTP/SSE connections; the daemon and Endpoint
continue running. `ui status` prints `running` or `stopped` and the URL (or `-` when stopped).
Both commands report an error when the daemon is stopped. Listener settings and running state are not persisted;
a daemon restart leaves WebUI stopped. For wildcard bindings, the URL shows the wildcard address;
use the node's reachable IP or hostname in a remote browser.

`ui init` does not start WebUI. `ui revoke-all` invalidates all browser sessions without stopping
WebUI. The old top-level `init`, `ui password set/reset`, `ui sessions revoke-all`, and `ui open`
commands are removed.

## Output and Errors

Commands with `--json` write the exact local API v1 envelope to standard output, except UI lifecycle
commands, which emit `{ "running": true, "url": "http://127.0.0.1:8080" }` or
`{ "running": false, "url": null }`. Diagnostics go to
standard error. Human output is intended for operators and may evolve; JSON discriminants and
fields are checked against the repository's golden schema.

Exit code `0` means success, `2` means command-line usage or secure-input rejection, and other
nonzero codes mean Runtime, IPC, persistence, network, credential, or operating-system failure.
