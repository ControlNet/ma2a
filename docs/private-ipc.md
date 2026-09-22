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

## Daemon State

Every lifecycle command begins by establishing what is actually there. `daemon.lock` is the only
ownership authority: a free lock means nothing owns the state directory, and a held lock means
something does, whatever the endpoint does or does not answer. A transport failure is never read as
proof that the lock owner has gone.

| State | What it means | What a command does |
| --- | --- | --- |
| `Absent` | The daemon lock is free. | `start` launches; `stop` reports that nothing is running. |
| `Ready` | The lock is held, a bounded lifecycle call was answered, and the daemon speaks this API version. | `start` is an idempotent success; `stop` uses the lifecycle plane. |
| `IncompatibleLifecycle` | As `Ready`, but the daemon speaks another business API version. | `start`/`restart`/`stop` stop it through the lifecycle plane. |
| `LegacyCurrentApi` | The lock is held by a daemon of this API version that predates the lifecycle plane. | `start` is an idempotent success; `stop` uses bounded `graceful_shutdown`. |
| `LegacyIncompatible` | The lock is held by a daemon of another API version that predates the lifecycle plane. | `start`/`restart`/`stop` use the bounded cross-version stop contract. |
| `Unresponsive` | The lock is held and no lifecycle call was answered. | Every command fails closed. Nothing is launched or unlinked. |
| `Conflicted` | Ownership and what answers the endpoint disagree. | Every command fails closed. Nothing is repaired by guessing. |

`Unresponsive` and `Conflicted` are never rounded down to `Absent`. Treating an owner that has
stopped answering as no owner at all is what allows a second Runtime to be started beside a wedged
first, and what allows a live daemon's socket to be unlinked underneath it.

## Verified Startup

`start` checks the state above, then starts the hidden `daemon-detached` entry from the current
executable when the directory is proven unowned. The new process uses a separate Unix session or
detached Windows process group. Each launch generates a random `launch_nonce` and passes it to the
child in the environment, never in argv.

The launcher keeps the child handle for the whole attempt. The daemon reports readiness as one
structured record on the pipe it inherited, carrying the protocol, the launch nonce, its process
identifier and creation stamp, the Runtime boot identifier and the Endpoint identity. The launcher
accepts that record only when the nonce is its own, so readiness from another launch proves nothing
about this one.

The readiness pipe is also the launcher's ownership lease. Until the record lands, the launcher
still owns the child; once it lands, the child owns itself. A daemon that cannot deliver its record —
because the launcher died, or the pipe is gone — stops rather than carrying on as an orphan.

If startup times out, the child exits, the record is malformed, the nonce does not match, or any
other pre-ready failure occurs, the launcher terminates the exact process it launched: a termination
request, a bounded wait, force if still alive, then a reap. A failed `start` therefore never leaves a
daemon behind.

Concurrent start/stop/restart requests serialize through the stable startup lock with bounded
exponential backoff; the daemon owns its separate lifetime lock.

The foreground `daemon` does not take the startup lock, so a launcher's verdict of `Absent` can be
overtaken by a foreground daemon claiming the directory before the launched child does. The launcher
therefore never removes an endpoint. The launched child either claims the daemon lock and only then
reclaims a stale endpoint, or finds the lock held and fails — and the launcher reports that failure
after reaping it — leaving the winner's endpoint untouched.

## Proven Teardown

`stop` does not treat an accepted shutdown as a completed one. The daemon keeps its lock file open
for the whole life of its process and never closes it deliberately, so the operating system releases
the lock as part of that process ending. Acquiring the lock is therefore the exit itself, reported by
the kernel, rather than an inference from a quiet socket. Only then are the endpoint and the daemon
record removed and `stop` reports success.

Inside the daemon, a graceful Runtime shutdown that does not finish within a bound is abandoned and
the process exits regardless, because a Runtime that will not close would otherwise hold the lock —
and every future start — indefinitely.

`restart` stops to proven absence and then starts. If teardown cannot be proven, it fails rather than
starting anyway. It holds the startup lock across both halves, preserving the state directory and
Endpoint identity, and also starts a stopped daemon. WebUI stays stopped after restart.

`status` only queries the Runtime snapshot. All business and UI commands fail if the daemon is
absent, without spawning or creating runtime state. The `daemon` subcommand remains in the foreground
for direct supervision, reports the same readiness record on standard output, and treats an interrupt
or a termination signal as graceful Runtime shutdown.

## Daemon Record

`run-v1/daemon.json` is an owner-private record of the launch in charge: protocol, API and binary
versions, process identifier and creation stamp, launch nonce, Runtime boot identifier, and Endpoint
identity. It is a witness, never an authority — `daemon.lock` decides ownership. A daemon removes the
record only while it still describes that daemon, so an older daemon can never erase a newer one's.

A process identifier alone is never treated as a process, because the operating system reuses it. It
is paired with the kernel's process creation stamp where the platform reports one (Linux and Windows),
and elsewhere the answer is "cannot tell" rather than a guess. Nothing destructive is ever authorized
by that answer: the lock decides ownership, and a launcher only ever terminates a child it still holds.

The default state directory is `$XDG_STATE_HOME/ma2a` on Unix when `XDG_STATE_HOME` is set,
`$HOME/.local/state/ma2a` otherwise, and `%LOCALAPPDATA%/ma2a` on Windows.

## Transport Contract

The transport carries the existing bounded local API v1 JSON unchanged. Each message uses a
12-byte header containing a four-byte big-endian payload length and an eight-byte big-endian
correlation identifier. Requests are limited to 16,384 bytes and responses to 65,536 bytes. The
server allows 32 in-flight business connections, plus a reserve of four that answer lifecycle calls
only, so it accepts at most 36 connections in total.

Two planes share this one endpoint and are kept logically apart. Business commands go through the
versioned local API, the durable mutation-replay lock and the Runtime actor. Lifecycle calls — "are
you there, which daemon are you" and "please stop" — are answered from an identity record the daemon
fixed before it began serving, and touch none of those. That separation exists because the moment a
caller most needs to ask whether a daemon is alive is the moment its Runtime is stuck, and a probe
that queues behind the stuck work answers nothing. The four reserve connections sit beyond the 32
business ones rather than inside them, so a lifecycle call is still answered when every business
slot is held by callers blocked on the same Runtime, and business traffic keeps its full budget.

The lifecycle envelope is deliberately not the local API envelope and carries no negotiation of its
own, because its shape must never change. A daemon that predates it replies with the local API's
error envelope, which a caller reads as exactly that and falls back to the versioned handshake.

One deadline bounds transport. Once a frame has begun arriving, the rest of it must complete within
two seconds; a peer that stalls mid-frame is malformed and returns `InvalidFrame`. The bytes of a
frame already exist, so there is no legitimate reason for that transfer to be slow, and the bound
also stops a stalled peer from holding a connection slot.

One further deadline bounds a complete lifecycle call, at five seconds. A lifecycle answer is
assembled from data the daemon already holds, so the only thing that can make one slow is a daemon
that is no longer able to serve — which is precisely the answer the caller is trying to obtain.
Waiting past that point cannot turn into a reply, so the wait becomes the `Unresponsive` verdict
instead of hanging the command that asked. Lifecycle waits are never made infinite to paper over a
lifecycle timeout.

For a business command, waiting for a reply to begin is deliberately unbounded. How long a Runtime takes to sign, commit,
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

A generous last-resort bound covers a child that neither reports readiness nor exits. A detached
daemon's standard error is captured in an owner-private file in the runtime directory, truncated for
each run, and a start that fails reports what the daemon itself recorded rather than only that a
deadline elapsed.

Business clients perform an exact API-version handshake before every non-handshake command.
Only explicit `start`, `stop`, and `restart` may recover from a version mismatch. A daemon that
answers the lifecycle plane is stopped through it whatever business API version it reports, because
that envelope never changes shape; its business version only decides whether business commands may
reach it. A daemon that predates the lifecycle plane falls back to the older contract: the lifecycle
client reads the daemon's advertised API version from a handshake or `version_mismatch` response
and sends only `graceful_shutdown` using that version and a fresh request ID. It validates the
response version, request ID, and `shutting_down` acknowledgement, then waits for the lifetime
lock to be released before replacement. That exchange runs on the business plane of a daemon this
executable cannot inspect, so the whole of it is bounded by a ten-second compatibility deadline; a
legacy daemon whose Runtime never acknowledges makes the command fail rather than hang. `stop` leaves the daemon stopped; `start/restart` launch
the executable used by the invoking CLI. Business requests never negotiate or retry another version.

Cross-version lifecycle compatibility requires retaining the private transport location, frame
format, response version field, and the `graceful_shutdown` request/response contract across API
versions. It uses the same current-user transport authorization as normal commands. A malformed
response, rejected stop, transport failure, or teardown timeout aborts replacement. There is no
PID-based termination or forced replacement of a daemon that has not released its lifetime lock.

## Privacy and Recovery

On Unix, runtime files live in `run-v1` with directory mode `0700`; `control.sock`, `startup.lock`,
`daemon.lock`, `daemon.log` and `daemon.json` use mode `0600`. The server rejects peers whose effective user ID differs from its
own. On Windows, pinned `interprocess` 2.4.3 leaves `accept_remote` false by default and therefore
creates each named-pipe instance with `PIPE_REJECT_REMOTE_CLIENTS`. The pipe also uses a protected
DACL granting access only to the current user's concrete SID and Local System. For every accepted
connection, the daemon captures its process-token SID, impersonates the named-pipe client, queries
the impersonated thread token's SID, reverts through an RAII guard, and accepts only an exact SID
match. Authorization never derives identity from a peer process identifier.

A stale Unix socket is removed only by a process that holds the daemon lock at that moment: a daemon
that has just claimed it and found nothing answering the endpoint, or a `stop` that has acquired it
after the daemon exited. A launcher that has merely observed the lock to be free removes nothing. A
failed probe is never on its own a licence to unlink an endpoint while the lock is held. A daemon
that refuses to start because something still answers the endpoint leaves that endpoint in place as
it exits. The daemon owns one Runtime instance, uses structured connection tasks with bounded
backpressure, and removes the endpoint it bound during graceful teardown while still holding the
singleton lock.
