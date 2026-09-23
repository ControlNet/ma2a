# Daemon generation transition lock

The lifecycle v2 follow-up fixes a stop/restart generation race: after a command classified daemon A, A could exit and a foreground `ma2a daemon` could claim the same state directory before the command sent its generic stop request. That request could stop the new daemon B.

`startup.lock` now serializes explicit start, stop, restart, and foreground daemon startup for one state directory. Foreground startup acquires it before `daemon.lock` and releases it after binding the endpoint and reporting readiness. The running daemon never reacquires it. An internally launched `daemon-detached` skips acquisition because its parent holds the transition lock through verified readiness. `daemon.lock` remains the sole lifetime ownership authority.

For `stop`, even the initial owner classification must occur under `startup.lock`; a pre-lock classification can become stale before the lock is acquired. A path existence check can preserve the no-state-creation behavior for never-used directories.

The debug-only rendezvous in `daemon_control/hold.rs` can pause after classification or before a replacement launch, and signal that a competing foreground daemon actually encountered a held `startup.lock`. Tests should wait for that contention signal, rather than use a timed sleep as evidence of the race.
