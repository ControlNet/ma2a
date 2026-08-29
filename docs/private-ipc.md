# Private IPC and Daemon Lifecycle

MA2A runs one foreground-capable daemon per current user and state directory. Commands that need
Runtime state connect through a private local transport and start the installed executable on demand
when no compatible daemon is live.

## Commands

```sh
ma2a [--state-dir PATH] status
ma2a [--state-dir PATH] shutdown
ma2a [--state-dir PATH] daemon
```

`status` performs an exact-version handshake, starts `ma2a daemon` from the current executable when
necessary, detaches the on-demand process into a separate Unix session or detached Windows process
group, and waits until the endpoint is live. Concurrent starters converge through a stable startup
lock with bounded exponential backoff before the daemon takes its separate singleton lock. `shutdown` waits until the
endpoint is removed and the singleton lock is released before it
returns success. The daemon subcommand remains in the foreground for direct supervision and handles
Ctrl-C as a graceful Runtime shutdown.

The default state directory is `$XDG_STATE_HOME/ma2a` on Unix when `XDG_STATE_HOME` is set,
`$HOME/.local/state/ma2a` otherwise, and `%LOCALAPPDATA%/ma2a` on Windows.

## Transport Contract

The transport carries the existing bounded local API v1 JSON unchanged. Each message uses a
12-byte header containing a four-byte big-endian payload length and an eight-byte big-endian
correlation identifier. Requests are limited to 16,384 bytes, responses to 65,536 bytes, and each
I/O operation has a two-second deadline. The server allows at most 32 in-flight connections.

Clients perform a version 1 handshake before every non-handshake command. Version negotiation and
fallback are intentionally unsupported.

## Privacy and Recovery

On Unix, runtime files live in `run-v1` with directory mode `0700`; `control.sock`, `startup.lock`,
and `daemon.lock` use mode `0600`. The server rejects peers whose effective user ID differs from its own. On Windows,
the namespaced pipe rejects remote clients, uses a protected DACL granting access only to the
current user's concrete SID and Local System, and rejects local callers whose process-token SID
does not equal the daemon user's SID.

A stale Unix socket is removed only after a process owns the startup lock and a liveness handshake
has failed. This prevents one contender from deleting a live daemon's endpoint. The daemon owns one
Runtime instance, uses structured connection tasks with bounded backpressure, and removes its
endpoint during graceful teardown while still holding the singleton lock.
