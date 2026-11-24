# Process Service v1

Phase 3 adds one Endpoint-targeted service family, Process. Exec answers "run this command
now". Process creates a workload that the target Endpoint hosts, and that remains manageable
after the operation that started it has ended. A Process is a resource, never an identity. It
has no Agent semantics and no scheduling.

Process reuses the Services v1 frame layer, status envelope, and caller-side stream plane
unchanged (`docs/protocol/services-v1.md`). This document only specifies what Process adds.

## Transport

The ALPN is exactly `ma2a/process/1`, and the service code in the `open` header is `7`
(`ServiceKind::Process`). It is negotiated like the Phase 2 ALPNs: only while the Runtime holds
at least one Space, and regardless of exposure.

- **Streams.** One operation uses one QUIC bidirectional stream, with the Services v1 frames
  `open`, `accept`, `reject`, `data`, `end`, `control`, `outcome`, and `cancel`.
- **Deadlines.** The `open` frame must arrive within 15 seconds. No other per-operation deadline
  applies. An `attach` lasts until the Process ends or the caller detaches.
- **Concurrency.** At most 16 streams per peer and 64 Runtime-wide are open at once. A stream over
  either bound is rejected with `concurrency_exceeded` before any byte is read.

`reject` means that nothing happened, and `outcome` means that the operation was accepted (see the
Services v1 caveats). Content-free operations send `accept` and `outcome` together, once the
operation has completed.

## Authorization

The authenticated QUIC peer is the caller. Every Process operation, not only `start`, needs
`ServiceAccess::Process`: **one single Space** in which

1. both the caller and this Runtime are current members;
2. the caller's signed member grant holds `process` (capability bit value `128`); and
3. this Runtime's local exposure for that same Space has kind `process` enabled (Store exposure
   kind `4`).

As in Services v1, grants and exposures from different Spaces never combine, and the authorizing
Space is never named or revealed. The check runs twice: once for coarse admission before the
`open` frame, and once after decoding it, both from durable state at call time. A denied caller
receives `reject(unauthorized)`.

**Creator-only visibility.** A remote caller sees and manages only the Processes that its own
Endpoint created. Any other ProcessId answers `unknown_process`, exactly like an ID that never
existed. A remote `list` returns only the caller's own Processes. The local operator sees and
manages every Process through the local API (`docs/protocol/local-api-v1.md`).

While an `attach` or `stdin` operation runs, it is re-authorized after every durable change (checked
every second). One that no longer passes ends with outcome `unavailable`, as on Runtime shutdown,
so the wire never tells a caller why it lost access. Only the target's local operation record
says `unauthorized`. Hosted workloads are ended separately; see
[Withdrawn authorization](#withdrawn-authorization).

## Identifiers

A **ProcessId** is 16 random bytes: a CBOR byte string of length 16 on the wire, and 32 lowercase
hexadecimal characters in text. A **RequestId** is the 16-byte `request_id` of the `open` header.

## Open bodies

The `open` header is the Services v1 map with service `7`. Its body (key `4`) is a tagged CBOR
array. Every value is canonical: definite lengths and shortest-form integers. Trailing items,
unknown tags, a wrong array length, and out-of-bound values are all `invalid_input`. The whole
header is at most 16 KiB.

| Tag | Operation | Body | Class | Streams |
| --- | --- | --- | --- | --- |
| 0 | `start` | `[0, start]` | mutation | none |
| 1 | `list` | `[1, after: ProcessId / null]` | read | none |
| 2 | `inspect` | `[2, id]` | read | none |
| 3 | `logs` | `[3, id, from, max_bytes: uint]` | read | out: `data` ch 1/2 |
| 4 | `attach` | `[4, id, from, stdin: bool]` | read; mutation if `stdin` | out: ch 1/2; in: ch 0 |
| 5 | `stdin` | `[5, id, close: bool]` | mutation | in: ch 0 |
| 6 | `signal` | `[6, id, signal: uint]` | mutation | none |
| 7 | `terminate` | `[7, id, grace_ms: uint]` | mutation | none |
| 8 | `delete` | `[8, id]` | mutation | none |

The class decides caller-side replay (see [Caller side](#caller-side-replay-and-lost-streams)).

### `start`

`start` is `[spec, stdin: bool, log_limit: uint, name: tstr / null, labels: [[key, value]…]]`,
and `spec` is `[program: tstr, args: [tstr…], cwd: tstr / null, env: [[name, value]…]]`.

| Field | Bound |
| --- | --- |
| `program` | required (not `null`), 1–4,096 bytes, no NUL; a bare name resolved through the target's `PATH`, or an absolute path |
| `args` | at most 256 typed arguments, each 0–4,096 bytes without NUL; nothing is interpreted by a shell |
| `cwd` | `null` (the target's home directory) or 1–4,096 bytes without NUL |
| `env` | at most 64 additions; `name` matches `[A-Za-z_][A-Za-z0-9_]*`, at most 128 bytes, unique; `value` 0–4,096 bytes without NUL |
| `stdin` | `true` keeps a stdin pipe for one writer at a time; `false` connects stdin to null |
| `log_limit` | retained output bytes, 1,048,576–16,777,216 (1–16 MiB); callers that name none send `DEFAULT_LOG_LIMIT` = 4,194,304 |
| `name` | `null` or 1–64 bytes without control characters |
| `labels` | at most 8; `key` matches `[a-z0-9][a-z0-9._-]*`, 1–64 bytes, unique; `value` 0–256 bytes without control characters |

A relative `cwd` is `reject(invalid_input)` before anything is recorded, as in Exec. An absolute
`cwd` that is missing on the target, like a program that cannot be spawned, is not rejected: the
start is accepted and the Process ends `failed` with reason `spawn_failed`.

### Positions and signals

`from`, in `logs` and `attach`, is one of:

| Encoding | Meaning |
| --- | --- |
| `[0]` | `start`: the oldest retained byte |
| `[1]` | `now`: the current end, so only output produced from now on |
| `[2, cursor: uint]` | this exact content-byte cursor |

`signal` is `interrupt` 0, `terminate` 1, `kill` 2, `hangup` 3, `quit` 4, `user1` 5, or `user2`
6 (SIGINT, SIGTERM, SIGKILL, SIGHUP, SIGQUIT, SIGUSR1, SIGUSR2). Other codes are `invalid_input`.

### Bounds

| Constant | Value |
| --- | --- |
| `MAX_LOGS_READ` | 1,048,576: `max_bytes` must be 1–1,048,576 |
| `MAX_TERMINATE_GRACE_MS` | 60,000: `grace_ms` must be 0–60,000 (there is no wire default) |
| `MAX_PROCESS_PAGE` | 32 summaries per `list` page |

## Accept payloads

| Form | Encoding | Sent by |
| --- | --- | --- |
| Started | `[0, id, replayed: bool]` | `start`; `replayed` is `true` when this RequestId already created the Process |
| Reading | `[1, state: tstr, start: uint, first_available: uint, end: uint, truncated: bool]` | `logs`, `attach` |
| Admitted | `[2]` | every other operation |

In Reading, `state` is the state *name* (such as `"running"`), and `start` is the cursor of the
first byte that follows. `first_available` and `end` are the retained window when reading began,
and `truncated` says that the requested cursor was older than `first_available`. A decoder
requires `first_available ≤ end` and `start ≤ end`.

## Outcome payloads

The outcome envelope is the Services v1 map `{0: status, 1: result / null}`. `status` is `0` for
success, and otherwise one of the status codes with a `null` result. A successful result is:

| Tag | Result | Encoding | Operation |
| --- | --- | --- | --- |
| 0 | Summary | `[0, summary]` | `start`, `signal`, `terminate` |
| 1 | Page | `[1, [summary…], next: ProcessId / null]` | `list` |
| 2 | Details | `[2, details]` | `inspect` |
| 3 | Logs | `[3, next: uint, ended: bool]` | `logs` |
| 4 | Detached | `[4, reason: tstr, summary, cursor: uint]` | `attach` |
| 5 | Written | `[5, bytes: uint, closed: bool]` | `stdin` |
| 6 | Deleted | `[6]` | `delete` |

`summary` is a 9-item array. It is content-free: no argv, environment, or output.

```text
[id: bstr(16), request_id: bstr(16), state: uint, reason: uint / null,
 termination / null, name: tstr / null, created_ms: uint, started_ms: uint / null,
 ended_ms: uint / null]
```

- **state** is `created` 0, `starting` 1, `running` 2, `exited` 3, `terminated` 4, `failed` 5, or
  `lost` 6.
- **reason** is `spawn_failed` 0, `interrupted` 1, `requested` 2, `unauthorized` 3, or
  `supervisor_lost` 4.
- **termination** is `[0, exit_code]`, `[1, signal_number]`, or `[2]` (unknown), as in Exec. It
  is present only when an end was observed.
- **times** are Unix milliseconds. `started_ms` is the time the supervisor reported the child
  spawned. `ended_ms` is set exactly when the state is terminal.

`details` is `[summary, start / null, first_available: uint, end: uint]`. `start` is the exact
`start` body the Process was created with, including its environment values, or `null` once the
Process is archived. A decoder requires `first_available ≤ end`.

The Detached `reason` is the Exec end-reason name: `completed` when the Process ended and every
retained byte was delivered, and `cancelled` when the caller detached.

## Operations

**`start`.** The target records the Process and its RequestId binding, sends `accept` Started,
brings the Process up, and then sends `outcome` Summary. The Summary is usually `running`, or
`failed(spawn_failed)` when the program could not start. It is `starting` when the supervisor's
answer to `go` was lost, and it may already be terminal for a very short program. See
[Start and RequestId binding](#start-and-requestid-binding). A Runtime that was started without
Process hosting refuses `start` with `unavailable`, but still serves every other operation on the
Processes it already hosts.

**`list`.** This returns one page of the caller's own Processes in ascending ProcessId byte order,
strictly after `after`. Archived tombstones are excluded. `next` is the last ID of the page when
more exist, or `null` otherwise.

**`inspect`.** This returns `details`. It also works for an archived Process, which then has
`start = null`.

**`logs`.** This reads the durable log directly, so it works in every state, including after the
Process and its supervisor have ended.

1. `from` resolves against the current window. `start` resolves to `first_available`, and `now`
   to `end`. A cursor above `end` is `reject(invalid_input)`. A cursor below `first_available`
   starts at `first_available` with `truncated = true`.
2. After `accept` Reading, at most `max_bytes` content bytes follow as `data` frames on channel 1
   (stdout) or 2 (stderr), in record order. They are read in batches of at most 256 KiB, and a
   record may be split at the budget boundary. The caller refuses an answer with more, and
   writes none of the frame that exceeds the budget.
3. If retention removed output while reading, a `control` Skipped frame reports the new cursor
   before any further data.
4. The outcome is Logs with `next`, the cursor to continue from. `ended` is `true` once the
   Process is terminal and `next` has reached the window's `end`.

**`attach`.** This follows a Process that is `starting` or `running`. Any other state is
`reject(not_running)`, and a terminal Process is read with `logs` instead.

1. `accept` Reading comes from the supervisor's live window. `from` resolves exactly as for
   `logs`, and a cursor above `end` is `reject(invalid_input)`.
2. Retained output from `start`, then live output, arrives as `data` on channels 1 and 2. A reader
   that falls behind retention receives a `control` Skipped frame and continues from the reported
   cursor.
3. The attachment ends in one of two ways:
   - when the Process has ended and every retained byte was delivered, the outcome is Detached
     `completed`;
   - when the caller sends `cancel`, closes, or loses its stream, or sends any frame it may not
     send, the attachment detaches with Detached `cancelled`.
   A caller that takes no output for 30 seconds ends it with a failed `timed_out` outcome. Input
   the Process does not take within 30 seconds detaches the attachment. A supervisor that has not
   taken the detach within 5 seconds, or reported 5 seconds after that, ends it with a failed
   `cancelled` outcome, so none of them holds the reader slot or the stdin lease.
4. The Detached summary is the Process as it now is, and `cursor` is the position reached.

With `stdin = true`, the attachment also takes the single stdin writer lease. `data` on channel 0
is written to the Process stdin. `end` on channel 0 closes the Process stdin permanently, and the
attachment keeps reading. Without the lease, any `data` or `end` detaches the attachment.

At most 8 readers are attached to one Process at a time. The ninth is
`reject(concurrency_exceeded)`.

**`stdin`.** This takes the writer lease on a Process that is `starting` or `running`. After
`accept` Admitted, the caller sends `data` on channel 0, then `end` on channel 0.

- With `close = true`, the Process stdin is then closed permanently.
- The outcome is Written, with the bytes delivered and whether the Process stdin is now closed.
- If the Process ends before the caller's `end`, the outcome comes at once, Written with
  `closed = true`; the caller need not end its input first.
- `cancel` or stream loss ends the operation with outcome `cancelled`. Bytes already delivered
  stay delivered.

**`signal`.** This needs a `running` Process, and is otherwise `reject(not_running)`.

- On Unix, the signal goes to the child's whole process group.
- Windows supports only `terminate` and `kill`, which end the child. Anything else is admitted
  and fails with outcome `invalid_input`.
- The outcome is the Summary as it was before the signal.
- A signal never changes the recorded cause. A Process that dies from a signal the caller sent
  ends `exited` with `[1, signal_number]`.

**`terminate`.** This sends `terminate` to the process group, waits up to `grace_ms`, and then
kills the group. The outcome Summary is sent only after the end has been recorded, usually as
`terminated(requested)`.

- An already terminal Process answers its current Summary.
- A `created` Process is `reject(not_running)`.
- A `starting` Process whose supervisor had not yet been told to spawn ends
  `failed(interrupted)`.
- A supervisor that is provably gone makes the Process `lost`, or decides it from the status
  record (see [Restart, reboot, and loss](#restart-reboot-and-loss)).
- A supervisor that does not answer within `grace_ms` plus 10 s is admitted and fails with outcome
  `unavailable`. Nothing is recorded, and a later `terminate` or withdrawal tries again.

**`delete`.** This archives a terminal Process. It removes the output directory and the stored
`start` (argv and environment), and keeps a tombstone: the ID, creator, RequestId, fingerprint,
final state, reason, termination, and times. A non-terminal Process is `reject(process_active)`.
Deleting an archived Process succeeds again with Deleted.

## Output log and cursors

The supervisor appends every chunk it reads from the child's stdout (channel 1) and stderr
(channel 2) as one record of 1–65,535 content bytes. In practice chunks are at most 32 KiB.

- **Cursor.** A cursor is the logical content-byte offset over both channels in record order. The
  cursor after a record is the cursor before it plus the record's length. A reader that knows its
  `start` therefore knows the cursor of every byte it receives.
- **Ordering.** Within one channel, the order is exact. Across channels, it is the order in which
  the supervisor received the chunks.
- **Window.** `first_available` is the first cursor still retained, and `end` is the cursor after
  the last complete record.
- **Retention.** When the retained content exceeds `log_limit`, whole oldest segments are
  deleted, but never the segment being written. Retention never blocks the child.
- **Skipped.** The `control` payload `[1, cursor: uint]` means that output before `cursor` was
  dropped by retention before this reader received it. The next byte has this cursor.

## State machine

The state is durable in the target's Store, and the Store refuses every transition that is not
listed here:

| From | To |
| --- | --- |
| `created` | `starting`, `failed` |
| `starting` | `running`, `exited`, `terminated`, `failed`, `lost` |
| `running` | `exited`, `terminated`, `lost` |

`exited`, `terminated`, `failed`, and `lost` are terminal. A `created` Process never becomes
`lost`, because no child can exist before `go`.

| State | Meaning | Reason |
| --- | --- | --- |
| `created` | row and RequestId binding committed; `go` was never sent | none |
| `starting` | committed before `go` is sent to the authenticated supervisor | none |
| `running` | the authenticated supervisor reported the child spawned | none |
| `exited` | the child ended by itself | none |
| `terminated` | MA2A ended it | `requested` (`terminate`) or `unauthorized` (withdrawal) |
| `failed` | no child ran | `spawn_failed` (program or supervisor could not start) or `interrupted` (no `go` was ever acted on) |
| `lost` | the supervisor is gone and recorded no end | `supervisor_lost` |

`running` is never inferred from a PID. It is asserted only by a supervisor that passed the
ownership proof on a live connection.

## Start and RequestId binding

The target's Store binds `(creator Endpoint, RequestId)` to one ProcessId with a unique key, and
records a fingerprint: BLAKE3 `derive_key("ma2a process start v1")` over the canonical `start`
body.

- **No binding yet.** The row (`created`), the binding, and the fingerprint commit in one
  transaction before any supervisor exists. The limits apply here:
  - at most 8 non-terminal Processes per creator, and 32 per Runtime;
  - at most 16 non-archived records per creator, and 128 per Runtime. The per-creator bound keeps
    one member's ended Processes from filling the Runtime for every other member; `delete` frees
    a record at once.

  A request over a limit is `reject(concurrency_exceeded)` with no effect.
- **Same fingerprint.** The target answers `accept [0, id, true]` and the Process as it is now.
  Nothing is spawned again, whatever the state, including for an archived tombstone.
- **Different fingerprint.** The request is `reject(conflict)`.

**Bring-up.**

1. The Runtime launches a supervisor and retries reaching it, through the ownership proof, for
   10 s. An attempt still running at that point may take its own 5 s deadline, so this lasts at
   most about 15 s. If it fails, the Process ends `failed(spawn_failed)`.
2. It commits `starting`.
3. It sends `go`.
4. It records the supervisor's report: `running`, or `failed(spawn_failed)`. If `go` got no
   answer, the follower reaches the supervisor again and, since the Process is still `starting`,
   sends `go` once more; a repeated `go` has no effect.

Bring-up runs even when the caller is gone before the `accept`: the workload never depends on a
connection, and a retry with the same RequestId answers it. A crash after the first commit can
therefore never produce a second Process for that RequestId.

### Caller side: replay and lost streams

Callers use the Phase 2 stream plane unchanged (`docs/protocol/local-api-v1.md`). Mutations
(`start`, `stdin`, `attach` with `stdin`, `signal`, `terminate`, `delete`) reserve their RequestId
in the caller Runtime's durable replay table. Reads never reserve.

- A completed operation's terminal frame is replayed verbatim to the same RequestId.
- A target `reject` releases the reservation.
- A stream lost after `open` was sent is settled as `outcome_unknown`, and that answer is also
  what a retry of the same RequestId receives.

The remediation for an unknown `start` is `process list`, whose summaries carry each Process's
`request_id`. The target-side binding guarantees that no duplicate is created, even for a caller
that retries the same RequestId through a fresh daemon or raw frames. Each `attach` with `stdin`
needs a fresh RequestId, because a reused one only replays the earlier outcome.

## Restart, reboot, and loss

The Runtime keeps one follower per non-terminal Process. The follower re-adopts the supervisor
only through the mutual ownership proof and watches its status reports.

**Evidence of loss.** Only two results count as proof that the supervisor is gone:

- *unreachable*: no socket or pipe, or a refused connection (which is what a stale Unix socket
  file gives);
- *unauthorized*: a peer that fails the proof, or on Unix runs as another user.

A timeout or a slow peer is never evidence. The follower retries with backoff from 250 ms up to
10 s, and also retries a Store failure instead of giving up. Every daemon request to a supervisor
has a deadline: `terminate` waits its grace period plus 10 s, and every other request waits 10 s. When the supervisor is proven gone, only its durable status record decides:

| Stored state | Supervisor proven | Supervisor proven gone |
| --- | --- | --- |
| `created` | send `abort`; the Process becomes `failed(interrupted)` | `failed(interrupted)` |
| `starting` | send `go` again (idempotent), then follow its reports | a recorded end decides; otherwise `lost(supervisor_lost)` |
| `running` | re-adopt and keep watching | a recorded end decides; otherwise `lost(supervisor_lost)` |

A recorded end maps to a state as follows:

| Recorded end | State |
| --- | --- |
| `ended` by itself | `exited` |
| `ended` by terminate | `terminated` |
| `spawn_failed` | `failed(spawn_failed)` |
| `never_started` | `failed(interrupted)` |

- **Daemon restart.** Reconciliation runs when the Runtime starts. A supervisor that outlived the
  daemon is re-adopted, and a `running` Process stays `running`.
- **Daemon shutdown.** Shutdown never stops a Process. It only closes the daemon's supervisor
  connections.
- **Host reboot.** Supervisors do not survive a reboot. Every non-terminal Process then finds no
  supervisor, and is decided from its status record as terminal, or becomes `lost`. It is never
  reported as running, and nothing is ever re-spawned.

## Withdrawn authorization

Once a second, if the Store revision has changed, every `starting` or `running` Process is
checked, and every 15 s even without a change. A Process whose creator no longer has one complete
authorization path (because of a revoked grant, a `process` unexposure, a departure, or a removal)
is terminated with a 5 s grace period and reason `unauthorized`.

- A Process that is already terminating is skipped.
- A termination that failed leaves nothing behind, so the next check tries again.
- A `created` Process is not checked: it has no child, and it is decided at bring-up or at the
  next start.

The same sweep restarts the follower of any live Process that lost it.

## Retention

- **Archiving.** A terminal Process is archived automatically 7 days after `ended_ms`, and its
  output directory is removed. The check runs every 60 s. `delete` archives immediately.
- **Tombstones.** A tombstone keeps the RequestId binding. Tombstones are pruned 30 days after
  archiving, and beyond the newest 1,024, oldest first. After pruning, the same RequestId can
  create a new Process.
- **Disk bound.** At most 128 non-archived records (16 per creator), each with at most
  `log_limit` retained output plus one open segment.

## Status codes

Process adds five codes to the Services v1 status set:

| Code | Name | Meaning | Produced by |
| --- | --- | --- | --- |
| 80 | `unknown_process` | not found, or not created by the caller | every operation that names an `id` |
| 81 | `not_running` | the operation needs a live Process | `attach`, `stdin` (not `starting`/`running`); `attach` with `stdin` and `stdin` before the child exists; `signal` (not `running`); `terminate` (`created`) |
| 82 | `writer_busy` | another attachment holds the stdin writer lease | `attach` with `stdin`, `stdin` |
| 83 | `stdin_closed` | the Process was started with `stdin = false`, or its stdin was ended | `attach` with `stdin`, `stdin` |
| 84 | `process_active` | the Process must be terminal first | `delete` |

Other codes used by Process:

| Code | Used for |
| --- | --- |
| `invalid_input` | malformed bodies, a relative `cwd`, `logs` or `attach` cursors above `end`, and unsupported signals on Windows |
| `unauthorized` | authorization of the `open` |
| `concurrency_exceeded` | stream limits, `start` limits, and the ninth reader |
| `conflict` | a RequestId reused with a different `start` body |
| `unavailable` | a Runtime without hosting, the Store, a supervisor that is slow to answer, and a running `attach` or `stdin` stopped by shutdown or withdrawn authorization |
| `io` | a log read failure, in `inspect` and `logs`; a Process with no log has an empty window instead |
| `cancelled` | a detached `stdin` operation |
| `outcome_unknown` | caller side only |

`start`, `list`, and `inspect` refuse only with `reject` frames. `signal`, `terminate`, and
`delete` `reject` only what is decided before any effect: `unknown_process`, `not_running`, and
`process_active`. Every later failure, including a supervisor that is gone, unproven, or slow, is
`accept` Admitted followed by a failed `outcome`, so a caller never mistakes a possible effect for
a refusal.

## Supervisor (informative)

The supervisor is local to the target host and never on the network. Each Process has its own
supervisor. It is the `ma2a` executable in a hidden supervisor mode, running
`ma2a_services::supervisor::supervise`, and it owns the child, the log, and the status record.

**Launch.**

- The daemon writes the launch record to the supervisor's stdin pipe, never to argv or the
  environment. The record is `"MA2APL1" ‖ ProcessId(16) ‖ secret(32)`, followed by the Process
  directory, the address, and the canonical `start`, each prefixed with its `u32` BE length.
- The supervisor detaches. On Unix it calls `setsid()`. On Windows it is created with
  `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP`.
- The 32-byte secret lives only in the owner-private Store row, where it is cleared when the
  Process becomes terminal, and in the supervisor's memory.

**Files.**

| Path | Content |
| --- | --- |
| `<state>/processes-v1/<id>/status` | atomic status record |
| `<state>/processes-v1/<id>/log/<first cursor, 20 digits>.seg` | log segments |
| Unix: `<state>/run-v1/p/<id>.sock` | socket, in a 0700 directory, at most 103 bytes |
| Windows: pipe `ma2a-p-<16 hex>-<id>` | the hex is the first 16 hexadecimal characters of BLAKE3 over the state directory as the file system resolves it (UTF-16LE), so every spelling of one directory names the same pipe, as for the daemon's own pipe |

`processes-v1` is also 0700. An existing socket name is never reclaimed.

**Startup order.** The supervisor creates the log and binds its socket *before* writing any
status record. It writes no initial `awaiting` record, and a missing record means awaiting (no
child was spawned).

**Ownership proof.** Every connection starts with a mutual proof in `control` frames:

1. The daemon sends a 32-byte nonce `Nd`.
2. The supervisor sends a nonce `Ns` followed by its MAC, `Ns ‖ MAC("supervisor")`, where
   `MAC(role) = BLAKE3-keyed(secret, "ma2a process supervisor proof v1" ‖ role ‖ ProcessId ‖ Nd ‖ Ns)`.
3. The daemon answers with `MAC("daemon")`.

Both sides compare the MACs in constant time. On Unix, both sides also require the peer's
effective UID to be their own. A reused PID, an unrelated program, or a stale socket file can
never pass the proof, and no PID is ever trusted.

**Requests.** After the proof comes one request per connection, as the payload of an `open`
frame:

| Request | Encoding |
| --- | --- |
| `watch` | `0` |
| `go` | `1` |
| `attach` | `2 ‖ from ‖ writer` |
| `stdin` | `3 ‖ close` |
| `signal` | `4 ‖ index` |
| `terminate` | `5 ‖ grace u32 BE` |
| `abort` | `6` |

In these encodings, `from` is `0` (start), `1` (now), or `2 ‖ u64 BE` (cursor).

**Two-phase start.** The child is spawned only after an authenticated `go`. Without `go` within
30 s, or on `abort` or `terminate` before `go`, the supervisor records `never_started` and exits.
`go` after the spawn is answered with the current status and no effect.

**Child.** The child runs in its own process group on Unix. Its stdout and stderr are piped into
the log, and its stdin is null or a pipe held for the lease. When the child ends, the rest of its
process group is killed. The pipes are then drained for at most 5 s in all, the log is synced, and
the status is recorded. Output a process outside the group writes after that drain is not logged.

**Status record.** The record is `"MA2AST1" ‖ tag ‖ fields` and is replaced atomically: written
to a temporary file, fsynced, renamed, and the directory synced.

| Tag | Status | Fields |
| --- | --- | --- |
| 0 | `awaiting` | none (never written as a first record) |
| 1 | `running` | pid u32, started_ms u64 |
| 2 | `ended` | termination kind u8, value u32, cause u8 (0 exited, 1 terminated), ended_ms u64 |
| 3 | `spawn_failed` | ms u64 |
| 4 | `never_started` | ms u64 |

The recorded PID is never used as evidence.

**Durability.** Records are written without per-record fsync, so they survive daemon and
supervisor crashes through the page cache. Segments are fsynced on rotation, at about 1 MiB of
file bytes, and at exit. After a host crash, only the unsynced tail may be missing.

**Exit.** After the final status, the supervisor lingers for at most 5 s while daemon connections
remain, then removes its Unix socket and exits.

## Known limitations

- **Orphans.** If a supervisor is killed, its child may keep running untracked as an orphan. The
  Runtime records `lost(supervisor_lost)`, and never claims or signals the child by PID.
- **Service managers.** A manager that kills a whole cgroup or job on daemon stop also kills the
  supervisors. With systemd, run the daemon with `KillMode=process`, so that supervisors outlive a
  daemon restart and are re-adopted.
- **Windows.** Only the direct child is killed. Only the `terminate` and `kill` signals exist. The
  supervisor pipe admits only Local System and the daemon's user, and the supervisor checks the
  client's user SID. A daemon cannot check the server's user, so on its side the ownership proof
  alone authenticates.
- **Restart during withdrawal.** The reason of an in-flight termination is held in memory. If the
  Runtime restarts while it ends a Process for withdrawn authorization, the re-adopted Process
  records `terminated(requested)`, not `terminated(unauthorized)`. Convergence still ends it.
- **Crashed target, waiting caller.** A target daemon that crashed never closed its QUIC
  connections. A caller learns this only at the QUIC idle timeout (30 s). Until then, its
  operations on the cached connection end `outcome_unknown`, even after the target has restarted
  and re-adopted its Processes. A target that stops gracefully closes its connections, so
  callers dial again at once.
- **Archive interrupted by a crash.** Archiving commits before the output directory is removed. A
  daemon crash between the two leaves that directory, at most `log_limit` plus one segment, until
  it is removed by hand. The tombstone and its binding are unaffected.
- **No terminal.** Processes use pipes. A terminal (PTY) Process is not part of v1.
- **Compatibility.** Pre-Phase-3 Runtimes accept member capability bits `0x7f` only. They cannot
  decode a signed Space member entry that carries `process` (bit value `128`), so a Space
  owner should grant `process` only once every member runs Phase 3.
