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
ma2a space create NAME [--json]
ma2a space list [--json]
ma2a space show SPACE_REF [--json]

ma2a space invite SPACE_REF [--ttl DURATION]
ma2a space accept
ma2a space leave SPACE_REF [--json]

ma2a space member remove SPACE_REF ENDPOINT_ID [--json]
ma2a space sync status --endpoint ENDPOINT_ID [--json]
ma2a space sync now --endpoint ENDPOINT_ID [--json]
```

### Space names and `SPACE_REF`

A Space keeps two identifiers. Its `SpaceId` is the immutable cryptographic identity; its name is
shared metadata signed into the Space's genesis body, so every Endpoint that enrolls reads the same
name without configuring anything locally. Names are not unique: two Spaces may both be called
`lab` as long as their Space IDs differ. Spaces created before names were persisted display their
canonical Space ID instead.

Every command that identifies a Space takes a positional `SPACE_REF`:

1. A complete Space ID resolves exactly.
2. Otherwise the reference is matched exactly (no fuzzy matching) against the shared names of the
   Spaces this Runtime belongs to.
3. No match reports that the Space was not found.
4. One match resolves automatically.
5. Several matches fail with an ambiguity error that lists the matching Space IDs; nothing is
   chosen for you, so pass the Space ID.

```sh
ma2a space show lab
ma2a space show 4b2d...
```

### Invitations

`ma2a space invite SPACE_REF` prints one single-use, owner-approved ticket to stdout and nothing
else, so it pipes and copies cleanly. `--ttl` accepts `ms`, `s`, or `m` between `1ms` and `5m` and
defaults to `5m`. Diagnostics go to stderr. Invite tickets are never accepted in argv and never
appear in JSON responses.

`ma2a space accept` reads one ticket without any flag. On an interactive terminal it prompts
`Invite:` and reads one line without echoing it; otherwise it reads the ticket from stdin:

```sh
ma2a --state-dir "$STATE_A" space invite lab | ma2a --state-dir "$STATE_B" space accept
```

### Leaving and removing members

`ma2a space leave SPACE_REF` is a membership operation, not a local deletion. The leaving Endpoint
asks the Space authority to sign the next manifest generation removing it, and persists only that
authority-signed state. If the authority cannot be reached the command fails and local membership
is unchanged. A Space's own owner cannot leave its Space; Phase 1 has no authority transfer.

`ma2a space member remove SPACE_REF ENDPOINT_ID` is the owner-side operation and remains a
cryptographic Space revocation. A removed Endpoint can rejoin later through a fresh valid invite.

Synchronization targets the peer Endpoint; the Runtime derives shared Spaces internally. `space
sync status` and `space sync now` are diagnostics: normal Space workflows never require triggering
control synchronization by hand.

### Errors

Human-readable commands report the Runtime's own protocol error, such as `invalid_input`,
`not_found`, `conflict`, `expired` or `unauthorized`, together with any remediation the Runtime
supplies. Secret material never appears in an error message.

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
