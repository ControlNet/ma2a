# Task 7 Private IPC Knowledge

## Reusable Design

- Keep transport semantics outside the transport: decode and dispatch through the existing typed
  local API, and frame only the encoded bytes plus a correlation identifier.
- A singleton daemon needs separate startup serialization and lifetime ownership. Check liveness,
  acquire a bounded nonblocking startup lock, check compatibility again, then inspect the daemon
  lock before reclaiming stale endpoints and spawning.
- Holding one lock in the parent and transferring ownership to the child leaves a spawn race. A
  separate startup lock can remain held until the child owns the daemon lock and answers handshakes.
- Preserve version mismatch as a typed probe result. Collapsing incompatibility into failed liveness
  turns an actionable error into a startup timeout.
- A graceful shutdown CLI should wait for both endpoint loss and singleton-lock release. An API
  acknowledgement alone does not prove the process has finished Runtime teardown.
- Keep the lock held through Runtime shutdown and endpoint removal. Releasing it first allows a new
  daemon to race with the old Runtime's cleanup.

## Security and Bounds

- Unix privacy requires directory permissions, endpoint permissions, and peer credentials together;
  any one of those alone is incomplete.
- Validate frame length before allocating payload storage, reject zero-length frames, and apply
  deadlines to header reads, payload reads, and writes.
- Bound accepted connection work with a semaphore and own all tasks in a `JoinSet` so daemon exit
  cannot detach request handlers.
- Never await a saturated semaphore inside the listener's selected accept branch. Reject excess
  accepted connections or select permit acquisition against cancellation.
- Share fingerprint-aware replay state across connections for mutations supported by the running
  daemon; durable restart replay remains a Runtime persistence responsibility.

## Verification Pattern

- Process-level tests with unique state directories can prove autostart, stale recovery, convergence,
  lock ownership, permissions, and complete shutdown without sleeps.
- Use bounded yielding around observable process transitions rather than fixed wall-clock sleeps.
- Drive the built binary manually after tests; this exposed the `Result` termination path printing
  enum debug output even though compile and lifecycle tests were green.
