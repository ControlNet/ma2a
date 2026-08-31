# Endpoint-Centric CLI

MA2A administration targets Endpoint IDs. Commands that require Runtime state use the private
current-user daemon and start it on demand. Use `--state-dir PATH` to isolate a Runtime; otherwise
MA2A uses the platform state directory described in [private-ipc.md](private-ipc.md).

## Runtime and Endpoint

```sh
ma2a init
ma2a status [--json]
ma2a endpoint show [--json]
ma2a daemon
ma2a shutdown
```

`init` securely prompts twice for the Web password. `daemon` runs in the foreground for direct
supervision. Runtime-requiring commands auto-start the same executable when no compatible daemon
is live.

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
ma2a ui password set
ma2a ui password reset
ma2a ui sessions revoke-all
ma2a ui open
```

Echo is Endpoint-addressed and has no Space routing option. Password commands read and confirm the
password without echo through a terminal; passwords are rejected from argv. `ui open` starts or
reuses the daemon, refuses when no password is configured, and launches a credential-free
`http://127.0.0.1:PORT` URL through the operating system process API without shell construction.

## Output and Errors

Commands with `--json` write the exact local API v1 envelope to standard output. Diagnostics go to
standard error. Human output is intended for operators and may evolve; JSON discriminants and
fields are checked against the repository's golden schema.

Exit code `0` means success, `2` means command-line usage or secure-input rejection, and other
nonzero codes mean Runtime, IPC, persistence, network, credential, or operating-system failure.
