# Phase 3 — Process supervisor design notes

## Layout

- `crates/ma2a-services/src/supervisor/`: the per-Process supervisor, which the `ma2a` binary runs
  in hidden mode.
  - `log`: segments, retention and cursor reads.
  - `status`: the atomic status record.
  - `proof`: the mutual keyed-BLAKE3 proof.
  - `address`: Unix socket or Windows pipe, plus connect.
  - `protocol`: supervisor requests.
  - `launch`: the stdin launch record and detached spawn.
  - `host*`: the supervisor main loop.
- `crates/ma2a-runtime/src/processes/`: the daemon side.
  - `start`: two-phase start.
  - `follow`: one follower per non-terminal Process.
  - `apply`: status → guarded Store transition.
  - `attach`: the relay of attach and stdin to the supervisor.
  - `reads`: list, inspect and daemon-side log reads.
  - `maintain`: reconcile, withdrawal convergence, expiry.
  - `host`: family dispatch plus signal, terminate and delete.
- Store jobs use the closure command `ServiceStoreCommand::Process(Box<dyn FnOnce(&mut Repository)>)`
  through `StoreClient::processes`. This avoids one enum variant per Process query.

## Pitfalls found

- **Frame payload bounds.** `accept` and `outcome` frames need at least 1 payload byte, and
  `FrameWriter::frame` refuses empty ones with `InvalidInput`. The supervisor uses
  `protocol::ACKNOWLEDGED = [0]` for content-free replies.
- **Supervisor exit.** The supervisor exits once the child has ended and no connection is left,
  after a linger of at most 5 s. A test or daemon that wants to read after the end must hold a
  connection, usually `watch`. Otherwise it reads the durable log files directly, which is what
  daemon-side `logs` does.
- **Slow fsync on this dev box.** Measured about 450 ms per fsync under a load average of about
  8.
  - Any fsync on an async worker, or before the socket is bound, can push past connect or ready
    deadlines.
  - Fixes:
    - Status writes and the log sync run in `spawn_blocking`.
    - The supervisor binds before writing any status record. A missing record means the same as
      `awaiting`.
  - Timeouts are never evidence of loss.
- **Evidence of loss.** Only definitive absence counts. `SupervisorAddress::connect` returns:
  - `Unreachable` for no endpoint (`NotFound`) or a refused connection (`ConnectionRefused`,
    which is what a stale Unix socket file gives);
  - `Unavailable` for a timeout or a slow peer;
  - `Unauthorized` for a failed proof.

  The follower concludes (status record, else `lost`) only on `Unreachable` or `Unauthorized`,
  and retries `Unavailable` with backoff.
- **`created` vs `starting` without a supervisor.** In `created`, `go` was never sent, so the
  Process is `failed(interrupted)`. In `starting` or `running` with no recorded end, it is
  `lost`, because a child may exist.
- **Clippy.** The at-most-3-arguments rule counts `self`, so a method takes `self` plus at most
  two parameters. Bundle them into tuples such as `(viewer, id)` or `((viewer, stop), (id, …))`.
- **Two RequestId replays, two answers.** A caller's daemon replays a mutation it already
  recorded, with the same `RequestId`, from its own replay table. The frame it replays is the
  recorded `outcome`, and no `accept`. So a CLI retry through the same daemon never shows
  `existing`. Only a retry that reaches the target sees `Started(id, existing = true)`, from the
  target's `(creator, request_id)` binding. Test that binding at the runtime level, as in
  `processes/tests/driver.rs`, not through two runs of the CLI.
- **Reject versus outcome.** Signal, terminate and delete `reject` only refusals made before any
  effect: `UnknownProcess`, `NotRunning`, `ProcessActive`. Every later error is
  `accept(Admitted)` followed by a failed `outcome`, so a caller never mistakes a possible effect
  for a refusal. That includes a supervisor that is `Unreachable` or `Unauthorized`, which goes on
  the wire as `Unavailable`.
- **Stop is Unavailable on the wire.** The stream host's `stop` token covers both shutdown and
  withdrawn authorization. Attach and stdin answer `Unavailable`, as Files does. Only the local
  operation report says `Unauthorized`.
- **Fault points use SIGKILL, not abort.** `ma2a_services::supervisor::crash()` serves
  `MA2A_TEST_PROCESS_FAULT` and `MA2A_TEST_SUPERVISOR_FAULT`, debug builds only. `abort()` hands
  the process to apport, the `core_pattern` pipe on this box, which holds it for seconds.
- **A crashed peer looks alive for 30 s.** `ma2a_net::ServiceClient` caches one QUIC connection
  per target and service.
  - Why it stays open: a SIGKILLed target never closes its connections. iroh draws a random
    stateless-reset key on every bind, so the restarted target cannot reset them either.
  - Effect: until the idle timeout (30 s), every caller operation on the stale connection hangs
    and then ends `outcome_unknown`.
  - In tests: crash E2E tests restart the owner while the caller is still blocked, and use
    `LONG_TICKER`, which outlives the timeout. `wait_until` retries errors until its 45 s
    deadline.
  - Each crash point gets a lab of its own, so no test reaches nextest's 120 s kill.
- **Non-follow logs read to a fixed end.** `process logs` stops at `window.end` from the first
  read's `Reading` accept. Chasing the live end never finishes for a Process that writes faster
  than one round trip, here a 100 ms ticker.
- **Finding supervisors in E2E.** `adversarial.rs::supervisors_of` scans `/proc/*/cmdline` for
  `__process-supervisor` plus the state directory (Linux only), to kill one and test
  `lost(supervisor_lost)`.
- **Web generated LOC.** `web/src/api/generated.ts` went past the 250-LOC policy. The dispatch
  helpers (`commandOperation`, `resultType`, `assertNever`) now live in `generated-dispatch.ts`
  and are re-exported, so imports elsewhere are unchanged.
- **Audit fixes before the freeze.** Each one closes a way a Process could get stuck:
  - Bring-up runs even when the caller vanished before the `accept`.
  - A failed `go` hands the follower no watch, so the follower re-sends `go`.
  - A failed terminate clears its `terminating` entry.
  - Convergence also sweeps every 15 s, which restarts lost followers and retries terminations.
  - The follower retries Store errors.
  - Supervisor requests have deadlines: `terminate` gets its grace plus 10 s.
  - Kill happens once after the grace period, with no busy loop.
  - Retained records are capped at 16 per creator.
  - The Windows supervisor pipe gets the private-IPC DACL plus a client SID check.
  - The SID FFI moved from `ma2a-runtime/src/ipc/` to `ma2a_services::windows_security`, so both
    crates share one audited copy.
  - Clippy on Windows needs `&raw mut` (`borrow_as_ptr`) for FFI out-params.
