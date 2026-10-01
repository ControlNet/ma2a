# ma2a-phase-3 — Persistent Process Layer

Baseline: Phase 2 freeze `e61a301` (Remote Capability Layer, frozen infrastructure).

## Goal

Phase 2 Exec answers "run this command now". Phase 3 Process answers "create a workload that
remains independently manageable after this client operation ends". A Process is a resource
hosted by the target Endpoint. It is never a cryptographic identity, and it has no Agent
semantics and no workflow scheduling.

## Architecture decisions

### P1 — Authority: one more signed bit, one more consent kind, creator ownership

- Member capability bit 7 (value 128) `process`, signed by the Space authority exactly like the
  Phase 2 bits. It defaults off.
- Per-Space local exposure kind `process` (enabled), next to `files_root`, `exec`, `shell`, and
  `proxy`. It is consent to host durable workloads for that Space's members.
- `ServiceAccess::Process`. Every Process operation, not only `start`, must find one complete
  Space where the caller holds `process` and the target exposes `process`. Grants and exposures
  from different Spaces never combine, and the Space is never disclosed.
- **Ownership.** A remote caller sees and manages only the Processes its own Endpoint created.
  Any other Process ID is `unknown_process`, exactly like an ID that never existed. This is
  resource ownership by Endpoint, not a new principal. The local operator sees and manages every
  Process through the local API.
- **Withdrawal converges.** After each durable change, and every 15 s, every `starting` or
  `running` Process whose creator no longer has a complete authorization path is terminated with
  reason `unauthorized`. A `created` Process has no child; it is decided at bring-up or restart. Revocation,
  grant removal, `process` unexposure, and departure all end hosted workloads, as consent ends
  with membership in Phase 2.

### P2 — Transport

- ALPN `ma2a/process/1` (`ServiceKind::Process`, wire code 7). It is negotiated like the other
  service ALPNs: only while the Runtime has at least one Space, and regardless of exposure.
- One operation is one stream with Services v1 frames. The local stream plane gains service
  `process`.
- Operations (the CBOR `open` body is tagged):

| Operation | Effect class | Streams |
| --- | --- | --- |
| `start {spec, stdin, log_limit, name?, labels}` | mutation | none |
| `list {cursor?}` | read | none |
| `inspect {id}` | read | none |
| `logs {id, from, max_bytes}` | read | data ch 1/2 |
| `attach {id, from, stdin}` | read, or mutation with `stdin` | data ch 1/2 out, ch 0 in |
| `stdin {id}` | mutation | data ch 0 in, `end` closes stdin |
| `signal {id, signal}` | mutation | none |
| `terminate {id, grace_ms}` | mutation | none |
| `delete {id}` | mutation | none |

Mutations reserve their RequestId in the Phase 1 durable replay table through the unchanged
Phase 2 stream-plane rules (D6). Reads never reserve.

### P3 — RequestId binds exactly one ProcessId

`ProcessId` is 16 random bytes, shown as 32 lowercase hex characters. The target's Store binds
`(creator Endpoint, RequestId)` to one ProcessId with a unique key and records the fingerprint
of the canonical `start` body.

- A `start` whose binding exists with the same fingerprint answers the existing Process. Nothing
  is spawned again, whatever state that Process is in.
- A different body answers `reject(conflict)`.
- The binding is inserted, in the same transaction as the Process row, before any supervisor
  exists. A crash anywhere after that commit can never produce a second Process for that
  RequestId.

The caller's own stream-plane replay row still turns a lost stream into `outcome_unknown`. The
remediation is `process list`, which shows each Process's RequestId. The target-side binding
guarantees "no duplicate" even for a caller that retries through a fresh daemon or raw frames.

### P4 — State machine (durable in SQLite)

```text
created ──► starting ──► running ──► exited | terminated | lost
   │            │
   └──► failed ◄┤
                └──► exited | terminated | lost
```

`created` never becomes `lost`. No `go` was sent, so no child can exist, and an unreachable
supervisor makes it `failed(interrupted)`. A start that already spawned may end before `running`
was recorded, so `starting` may move straight to an end.

| State | Meaning |
| --- | --- |
| `created` | Row and RequestId binding committed. No `go` was ever sent, so no child exists. |
| `starting` | Committed before `go` is sent to an authenticated supervisor. |
| `running` | The authenticated supervisor reported the child spawned (pid, start time). |
| `exited` | The child ended by itself: exit code or signal. |
| `terminated` | MA2A ended it: `terminate`, or withdrawn authorization (`unauthorized`). |
| `failed` | The program could not start (`spawn_failed`), or the start was interrupted before `go`. |
| `lost` | MA2A cannot prove the outcome: the supervisor is unreachable and recorded no end. |

`running` is never inferred from a PID. It is asserted only by an authenticated supervisor on a
live connection, both at start and after every daemon restart. Only these transitions are legal;
the Store refuses any other.

### P5 — Supervisor architecture (ownership proof, no PID trust)

Each Process has its own supervisor: the same `ma2a` executable in the hidden mode
`ma2a __process-supervisor`. The supervisor owns the child, the durable log, and the terminal
status record.

- **Launch.** The daemon writes the launch record (Process dir, socket name, 32-byte secret,
  spec, stdin policy, log limit) to the supervisor's stdin pipe, never to argv or environment.
  The supervisor then detaches:
  - Unix: `setsid()` makes it a new session leader outside the daemon's process group and
    terminal;
  - Windows: `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP`.

  It survives daemon exit and crash. Service managers that kill a whole cgroup or job need
  `KillMode=process`-style configuration, which is documented.
- **Socket.** The supervisor listens on a per-Process local socket:
  - Unix: `<state>/run-v1/p/<id>.sock` in the owner-private (0700) runtime directory, with a
    peer euid check;
  - Windows: a namespaced pipe `ma2a-p-<16 hex of blake3(<state>/processes-v1)>-<id>` with the
    owner-only security descriptor used by private IPC (`ma2a_services::windows_security`), and
    a client user-SID check on the supervisor side.
- **Ownership proof.** Every connection starts with a mutual challenge. Each side sends a
  32-byte nonce and proves knowledge of the Process secret with keyed BLAKE3 over a
  domain-separated transcript (domain ‖ role ‖ ProcessId ‖ daemon nonce ‖ supervisor nonce;
  see `docs/protocol/process-v1.md`). The secret lives only in
  the owner-private Store row and the supervisor's memory. An unrelated process, a reused PID, or
  a stale socket file can never pass. A supervisor is re-adopted only through this proof.
- **Two-phase start.** The supervisor spawns the child only after an authenticated `go`. The
  daemon sends `go` only after committing `starting`. A supervisor without `go` within 30 s writes
  `never_started` and exits, so a daemon crash before `go` can never leave an unmanaged child.
- **Child.** The child gets its own process group on Unix (whole-group signals), and stdout and
  stderr pipes into the log. Stdin is null, or a pipe the supervisor holds for the single writer.
  The supervisor is the child's parent and reaps it, so the child's PID cannot be reused while
  the supervisor lives.
- **Status record.** `<process dir>/status` is replaced atomically (temp, fsync, rename):
  `running {pid, started_ms}`, `ended {termination, reason, ended_ms}`, `never_started`, or
  `spawn_failed`. The supervisor binds its socket before writing any status record and writes no
  initial `awaiting` record; a missing status record means awaiting. After the child ends and the
  log and status are durable, the supervisor sends `ended` to its watchers and exits.
- **Supervisor protocol.** Services v1 frames (the Phase 2 `FrameReader` and `FrameWriter`) over
  the local socket, one request per connection after the proof: `watch`, `go`,
  `attach {from, writer}`, `stdin`, `signal`, `terminate {grace}`, `abort`. There is no second
  stream implementation.

### P6 — Durable output (bounded, cursor-addressed)

- The supervisor appends every chunk it reads from stdout (channel 1) or stderr (channel 2) as a
  record `channel u8 ‖ len u32 ‖ bytes` (each chunk ≤ 64 KiB) to segment files
  `<process dir>/log/<first cursor, 20 digits>.seg`. Segments rotate at 1 MiB.
- **Cursor** = the logical content-byte offset over both channels in record order. The cursor
  after a record = the cursor before it + its byte length. A reader that knows its start cursor
  knows every position from the bytes it received.
- **Ordering.** Records keep the order the supervisor received them. Within one channel the order
  is exact. Across channels it is the order the chunks arrived, which is the only order an
  observer can know.
- **Retention.** `log_limit` per Process is 1–16 MiB (default 4 MiB). When the retained bytes
  exceed the limit, whole oldest segments are deleted. The child is never blocked by retention.
  `first_available` is the first cursor still held.
- **Overflow.** A `logs` or `attach` request from a cursor below `first_available` starts at
  `first_available` with `truncated = true`. A live attach reader that falls behind retention
  receives `control {skipped_to}` and continues.
- **Durability.** Records are written without per-record fsync. They survive daemon and
  supervisor crashes (page cache). Segments are fsynced on rotation and at exit. After a host
  crash only the unsynced tail may be missing.
- **Disk bound.** At most 128 retained Process records per Runtime (16 per creator), so logs are
  bounded by 128 × 16 MiB plus small state files.

### P7 — Attach (multiple readers, single writer)

- `attach {id, from: start | cursor N | now, stdin}` → `accept {state, cursor, first_available,
  truncated}`. The caller then receives history from `from`, followed by live records, as
  `data` channels 1 and 2. When the Process has ended and every retained byte was delivered, the
  terminal frame is `outcome {state, termination, end_cursor}`.
- A caller `cancel` or stream loss only detaches (`outcome` reason `cancelled`). The Process
  continues.
- At most 8 readers per Process.
- **Writer lease.** At most one stdin writer at a time: `attach` with `stdin`, or `stdin`. A
  second writer gets `reject(writer_busy)`. Detaching the writer releases the lease but does not
  close the Process stdin. Only an explicit `end` on channel 0 closes it, permanently.
- Unlike Shell, the workload exists independently of every attachment. Phase 3 Processes use
  pipes. A terminal (PTY) Process is deferred and documented.

### P8 — Restart recovery and reboot

The reconciler runs at Runtime start and after each durable change. For every non-terminal
Process:

| Stored | Supervisor proof passes | Supervisor unreachable |
| --- | --- | --- |
| `created` | send `abort` → `failed(interrupted)` | → `failed(interrupted)` (no `go` was ever sent) |
| `starting` | send `go` again (idempotent), then follow its report | status record decides, else `lost` |
| `running` | re-adopt as `running` and resume watching | status record `ended` → `exited`/`terminated`; else `lost` |

- **Machine reboot.** Supervisors do not survive a host reboot. After a reboot every
  non-terminal Process reopens from its status record as terminal, or as `lost`. It is never
  reported as running. An end the status record holds wins over `lost`: a child that ended,
  recorded its end, and then lost its supervisor in the reboot keeps that end.
- **Supervisor loss.** If the supervisor is killed, its child may keep running untracked as an
  orphan. MA2A reports `lost` and never claims or signals it by PID. This is documented.
- **Daemon shutdown** does not stop Processes. It closes the daemon's supervisor connections.

### P9 — Management and retention

- **`signal`.** The signal is typed: `interrupt`, `terminate`, `kill`, `hangup`, `quit`, `user1`,
  or `user2`. It is sent to the Process group on Unix. Windows supports only `kill` and
  `terminate`; anything else is `invalid_input`. A non-running Process answers `not_running`.
- **`terminate {grace_ms ≤ 60 s}`** (the CLI default is 10 s) sends terminate to the group, waits
  up to the grace period, then kills once. It records reason `terminated`, and its outcome follows the
  Process end.
- **`delete`** archives a terminal Process. It removes the log, the status record, and the spec
  (argv, env). It keeps a tombstone (id, creator, RequestId, fingerprint, final state, times), so
  a retried `start` still answers that Process instead of creating a new one. A non-terminal
  Process answers `process_active`.
- **Retention.** A terminal Process is archived automatically 7 days after it ended. Tombstones
  are pruned oldest first beyond 1,024, or 30 days after archive.
- **Limits.** At most 8 non-terminal Processes per creator, 32 per Runtime, and 16 retained
  (non-archived) records per creator, 128 per Runtime. Excess `start` requests get `reject(concurrency_exceeded)` with no
  effect.

### P10 — New errors

| Code | Name | Meaning |
| --- | --- | --- |
| 80 | `unknown_process` | not found, or not owned by the caller |
| 81 | `not_running` | the operation needs a running Process |
| 82 | `writer_busy` | another attachment holds the stdin writer |
| 83 | `stdin_closed` | stdin policy `none`, or stdin already ended |
| 84 | `process_active` | the Process must be terminal first |

## Local API and CLI

- **Local API.**
  - `process_list` and `process_inspect` show every Process: creator, RequestId, state, times,
    termination, and log cursors. The spec is shown to the local operator.
  - `process_terminate` and `process_delete` are local mutations with RequestId replay.
  - `process` joins `service_expose` and `service_unexpose`; `space_member` gains the `process`
    grant.
- **CLI (remote).**

  ```text
  ma2a process --endpoint ID start [--name N] [--label K=V] [--cwd D] [--env K=V] [--stdin]
      [--log-limit BYTES] -- PROGRAM ARGS…
  ma2a process --endpoint ID list|inspect ID|logs ID [--from C] [--follow]|attach ID [--stdin]
      [--from start|now|C]|stdin ID|signal ID SIGNAL|terminate ID [--grace T]|delete ID
  ```

  Without `--endpoint`, `list|inspect|terminate|delete` use the local API.

## Crate layout

- **`ma2a-core`**
  - `ServiceKind::Process`, the `process` capability bit and exposure kind, and
    `ServiceAccess::Process`;
  - `service/process/*`: ProcessId, requests, accepts, outcomes, states, signals, and cursors,
    as canonical CBOR.
- **`ma2a-store`**: schema 8. `service_exposures` kind 4; a `processes` table with the unique
  `(creator, request_id)` binding and guarded transitions.
- **`ma2a-services`**: the `ma2a_services::supervisor` module (no separate crate): log segments
  (writer, retention, reader), status record, ownership proof, supervisor protocol client and
  server, supervisor main loop, and platform launch.
- **`ma2a-net`**: `PROCESS_ALPN`.
- **`ma2a-runtime`**:
  - process manager: supervisor watchers, reconciler, withdrawal convergence;
  - Process family host in `stream_host`;
  - local API commands; Store client.
- **`ma2a-app`**: the hidden supervisor subcommand, the `process` CLI, and E2E.
- **`web`**: a read-only Processes table on the Services route.

## Deterministic fault points

These are debug builds only, as with `MA2A_TEST_HOLD_*`. A fault point kills its own process with
`SIGKILL`, as a crash would; `abort` would wait for the system's core-dump handler.
`MA2A_TEST_PROCESS_FAULT=<point>` makes the daemon crash at:

- `after_created` (row committed, no supervisor);
- `after_launch` (supervisor authenticated, `starting` not committed);
- `after_go` (child spawned, `running` not committed);
- `before_response` (`running` committed, outcome not written).

`MA2A_TEST_SUPERVISOR_FAULT=before_status` makes a supervisor crash after its child ends and
before the status record, which proves `lost`. Unit tests drive the same windows in-process.

## Acceptance gate (real multi-Endpoint E2E)

- Start a Process, disconnect the client, and the Process continues.
- A new client lists and inspects it and reads the retained logs.
- Attach to the running Process (stdin and output). Attach disconnect leaves it running.
- Terminate it.
- Retry the start with the same RequestId: no duplicate.
- Restart the daemon: the running Process is rediscovered and reconciled safely.

The adversarial tests cover:

- PID reuse: an orphan child alive after supervisor loss is `lost`, never `running`, and an
  impostor socket without the secret is refused;
- supervisor loss;
- a daemon crash at each fault point;
- log retention overflow;
- attach disconnect;
- withdrawn consent;
- unrelated, non-owner, and revoked callers;
- limits.

Phase 1 and Phase 2 E2E remain green.

## Freeze record (as built)

Phase 3 is frozen on local `master`. The commits after the Phase 2 freeze (`e61a301`) are:

- `build`: optimized crypto crates in debug builds, needed for the test time budget;
- `feat(core)`, `feat(services)`, `feat(runtime)`, `feat(app)`, `feat(web)`: Phase 3 itself;
- `fix(store)`, `fix(services)` ×2, `fix(runtime)`, `test(app)`: fixes from the freeze audit;
- this documentation commit.

**Audit fixes.** The fresh audit found 7 should-fix issues and no blockers. All 7 are fixed:

- a start whose caller vanished before the accept stayed `created`;
- a failed `go` was never re-sent;
- a stuck `terminating` mark blocked convergence;
- followers died on Store errors;
- the supervisor busy-looped after the kill, and supervisor requests had no deadlines;
- one creator could fill the retained-record cap;
- the Windows supervisor pipe had the default DACL.

The protocol document was corrected where it disagreed with the code:

- a relative `cwd` is `invalid_input`;
- a withdrawn attach is `unavailable` on the wire;
- the reject-versus-outcome rule;
- `stdin_closed` and `not_running`;
- the convergence scope and sweep;
- the limits and deadlines.

**Verification at freeze**, on Linux x86_64:

| Check | Result |
| --- | --- |
| `cargo fmt --all -- --check` | clean |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | clean |
| `cargo xtask check-loc` | clean |
| `cargo deny check` | clean |
| `cargo machete` | clean |
| `cargo nextest run --workspace --all-features` | 791/791 passed |
| `-E 'binary(process_e2e) \| binary(services_e2e) \| binary(e2e)'`, run again | 91/91 passed |
| `ma2a-services` clippy for `aarch64-apple-darwin` and `aarch64-pc-windows-msvc` (`--features blake3/pure`) | clean |
| web: `biome check`, `tsc --noEmit`, `bun test` | clean, 138 tests passed |

Crates that link `ring` or SQLite (`ma2a-runtime`, `ma2a-app`) cannot be cross-built on this host. They rely on native CI for macOS and Windows.

**Test-only surfaces**, all highlighted, none in release behaviour:

- `ProcessHosting::in_process()` is `#[cfg(test)]`.
- `MA2A_TEST_PROCESS_FAULT` and `MA2A_TEST_SUPERVISOR_FAULT` work in debug builds only.
- `web/src/test/services-fixture.ts` is test data.

**Known limitations** are listed in `docs/protocol/process-v1.md`:

- orphans after a supervisor is killed;
- `KillMode=process` under systemd;
- Windows ends only the direct child;
- no PTY;
- the reason is lost when a restart interrupts a withdrawal;
- a caller notices a crashed target only at the 30 s QUIC idle timeout;
- a crash between archiving and directory removal leaks that directory.
