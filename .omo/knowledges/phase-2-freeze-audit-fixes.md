# Phase 2 freeze audit: containment, session reaping, live re-authorization

## Files containment must hold handles, not names

`lstat` of each ancestor followed by an operation on the full path is a TOCTOU race that a remote
caller can win by itself, using the allowed `rename` of a symlink or of a directory holding one.
`crates/ma2a-services/src/files/resolve.rs` now walks with held directories
(`files/dir_unix.rs`: `openat(O_DIRECTORY|O_NOFOLLOW)` plus `statat/openat/mkdirat/unlinkat/
renameat/linkat` relative to the parent fd; `files/dir_windows.rs`: every ancestor pinned by a
handle opened without `FILE_SHARE_DELETE` and with `FILE_FLAG_OPEN_REPARSE_POINT`, no new unsafe).
Regression: `resolve.rs::a_swapped_ancestor_never_redirects_a_held_operation`.
Details that matter:
- Errno for a symlink under `O_NOFOLLOW|O_DIRECTORY` differs by platform; classify by `statat`.
- Open files with `O_NONBLOCK` (then clear it) so a FIFO swapped in cannot block the thread, and
  re-check the opened fd's metadata.
- `rustix::fs::Stat` field widths differ by target; widen through `i128::from` instead of casts.
- `mkdir -p` must validate names without walking (ancestors do not exist yet).

## Shell sessions must reap the whole terminal session

A job-control shell puts `&` jobs in their own process groups, so a group kill misses them, and
the output reader thread stays blocked while any of them holds the slave. `shell/reap.rs`:
SIGHUP the leader group, 100 ms grace if the leader still runs, SIGKILL the group, then on Linux
kill every non-zombie `/proc/*/stat` whose session field equals the leader PID (bounded sweeps).
Regression (Linux): `tests/shell_host.rs::background_jobs_never_outlive_their_session` with
`sh -c 'set -m; sleep 300 & …'`. After a failed run, kill leftovers with
`pkill -KILL -f '^sleep 300$'` (never `pkill -f` a pattern contained in your own command line: it
kills the invoking shell, exit 144).

## Authorization must hold while an operation runs

`crates/ma2a-runtime/src/stream_host/reauthorize.rs` keeps each running inbound operation's
`(peer, access, bound root, stop token)`. A 1 s task reads the store revision *before* snapshotting
the registry, and when it moved, re-runs `authorize_service` for each; a failure or a different
root cancels the operation's child token (Exec/Shell/Proxy see it as shutdown; Files/Transfer are
reset). Operations register *before* their own authorization so no change can fall between.
Regression: `services_e2e lifecycle::withdrawn_consent_or_membership_ends_running_sessions`
(verified to fail with the re-check task disabled).

## Smaller ones

- Blocking Files jobs hold a permit of a static 16-slot semaphore inside `spawn_blocking`.
- Proxy upstream keeps reading after the caller's `end` until the destination side delivered,
  so a later `cancel`/reset is seen (`proxy_host::a_cancel_after_end_stops_…`).
- Stream-plane relay: `reject` after `accept` is `Terminal::Lost`, never forwarded.
