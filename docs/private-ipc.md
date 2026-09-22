# Private IPC and Daemon Lifecycle

MA2A runs one foreground-capable daemon per current user and state directory. Commands that need
Runtime state connect through a private local transport and report an error when no compatible
daemon is live. Only explicit lifecycle commands start a daemon.

## Commands

```sh
ma2a --state-dir "$STATE" start
ma2a --state-dir "$STATE" restart
ma2a --state-dir "$STATE" stop
ma2a --state-dir "$STATE" status
ma2a --state-dir "$STATE" daemon
```

`start` checks for an existing exact-version daemon, then starts the hidden `daemon-detached`
entry from the current executable when needed. The new process uses a separate Unix session or
detached Windows process group. The command returns once private IPC is ready. Repeated starts
reuse the same protocol-compatible live daemon. An incompatible daemon is gracefully stopped
before launching the current executable. Concurrent start/stop/restart requests serialize through the stable
startup lock with bounded exponential backoff; the daemon owns its separate lifetime lock.

`stop` requires a running daemon and waits until its endpoint is removed and lifetime lock is
released. `restart` holds the startup lock across stop and start, preserving the state directory
and Endpoint identity; it also starts a stopped daemon. WebUI stays stopped after restart.
`status` only queries the Runtime snapshot. All business and UI commands fail if the daemon is
absent, without spawning or creating runtime state. The `daemon` subcommand remains in the
foreground for direct supervision and handles Ctrl-C as graceful Runtime shutdown.

The default state directory is `$XDG_STATE_HOME/ma2a` on Unix when `XDG_STATE_HOME` is set,
`$HOME/.local/state/ma2a` otherwise, and `%LOCALAPPDATA%/ma2a` on Windows.

## Transport Contract

The transport carries the existing bounded local API v1 JSON unchanged. Each message uses a
12-byte header containing a four-byte big-endian payload length and an eight-byte big-endian
correlation identifier. Requests are limited to 16,384 bytes and responses to 65,536 bytes. The
server allows at most 32 in-flight connections.

Two deadlines apply, because transferring a frame and executing a command are different things.
Once a frame has begun arriving, the rest of it must complete within two seconds; a peer that
stalls mid-frame is malformed and returns `InvalidFrame`. Waiting for a reply to begin is instead
bounded by the command's own deadline: twenty seconds for a local command, and ninety seconds for
an operation that legitimately performs bounded remote work (`space_redeem`, `space_leave`,
`space_invite`, `echo_call`, `control_sync_trigger`), which stays clear of the thirty-second
enrollment and control-round exchanges and the ten-second Echo deadline underneath it. Exceeding a
command deadline reports a command timeout, never malformed framing, so a Runtime still committing
valid work is never misreported as a broken transport.

Business clients perform an exact API-version handshake before every non-handshake command.
Only explicit `start`, `stop`, and `restart` may recover from a version mismatch: the lifecycle
client reads the daemon's advertised API version from a handshake or `version_mismatch` response
and sends only `graceful_shutdown` using that version and a fresh request ID. It validates the
response version, request ID, and `shutting_down` acknowledgement, then waits for the lifetime
lock to be released before replacement. `stop` leaves the daemon stopped; `start/restart` launch
the executable used by the invoking CLI. Business requests never negotiate or retry another version.

Cross-version lifecycle compatibility requires retaining the private transport location, frame
format, response version field, and the `graceful_shutdown` request/response contract across API
versions. It uses the same current-user transport authorization as normal commands. A malformed
response, rejected stop, transport failure, or teardown timeout aborts replacement. There is no
PID-based termination or forced replacement of a daemon that has not released its lifetime lock.

## Privacy and Recovery

On Unix, runtime files live in `run-v1` with directory mode `0700`; `control.sock`, `startup.lock`,
and `daemon.lock` use mode `0600`. The server rejects peers whose effective user ID differs from its
own. On Windows, pinned `interprocess` 2.4.3 leaves `accept_remote` false by default and therefore
creates each named-pipe instance with `PIPE_REJECT_REMOTE_CLIENTS`. The pipe also uses a protected
DACL granting access only to the current user's concrete SID and Local System. For every accepted
connection, the daemon captures its process-token SID, impersonates the named-pipe client, queries
the impersonated thread token's SID, reverts through an RAII guard, and accepts only an exact SID
match. Authorization never derives identity from a peer process identifier.

A stale Unix socket is removed only after a process owns the startup lock and a liveness handshake
has failed. This prevents one contender from deleting a live daemon's endpoint. The daemon owns one
Runtime instance, uses structured connection tasks with bounded backpressure, and removes its
endpoint during graceful teardown while still holding the singleton lock.
