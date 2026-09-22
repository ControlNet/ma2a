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

One deadline applies, and it bounds transport only. Once a frame has begun arriving, the rest of
it must complete within two seconds; a peer that stalls mid-frame is malformed and returns
`InvalidFrame`. The bytes of a frame already exist, so there is no legitimate reason for that
transfer to be slow, and the bound also stops a stalled peer from holding one of the 32 connection
slots.

Waiting for a reply to begin is deliberately unbounded. How long a Runtime takes to sign, commit,
or reach a peer is not a transport property, and no duration placed here can distinguish work that
is still progressing from work that never will. The exact signal already exists: a Runtime that
stops closes its socket, and the unbounded read then ends immediately with end-of-file. A Runtime
that can no longer answer commands exits rather than lingering, so that signal covers a stopped
Runtime and a stalled one alike. Earlier revisions guessed a per-operation budget and reported
failures for commands the daemon was still committing; no client-side command deadline remains.

Remote work keeps its own deadlines inside the Runtime, where they are necessary: a peer Endpoint
can stall forever without ever closing anything. Enrollment, departure, and control-round exchanges
allow thirty seconds and Echo allows ten.

The loopback HTTP adapter adds one bound of its own, because an HTTP request must be answered or
refused rather than held open. It is a single generous limit applied at that boundary, far longer
than any Runtime operation and never set per operation.

A detached daemon reports readiness on a pipe it inherits from the process that started it: one
line once its endpoint is bound and it can serve commands. The starter therefore returns as soon as
the daemon is genuinely ready, learns of a daemon that died during startup through end-of-file, and
reports that daemon's own recorded reason instead of a timeout. A generous last-resort bound covers
a child that neither reports readiness nor exits, and the starter terminates the child it spawned
rather than leaving it without an owner. A detached daemon's standard error is captured in an
owner-private file in the runtime directory, truncated for each run.

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
