# Todo 7 Private IPC

- Implemented in isolated worktree `/tmp/opencode/ma2a-todo-7` from exact base
  `d9e4130465a163acd800d0bd39abb5ba24d90262` on branch `todo-7-private-ipc`.
- The transport reuses local API v1 commands and encoded responses with a bounded correlation
  frame, exact-version handshake, two-second I/O deadlines, 32-connection backpressure, and
  structured `JoinSet` teardown.
- Unix uses an owner-only `run-v1` directory, mode `0600` socket and lock files, and peer effective
  UID authorization. Windows uses a namespaced pipe with an owner-and-System protected DACL.
- First-use commands converge through a stable standard-library file lock. Stale endpoint removal
  occurs only under that lock after liveness fails.
- The daemon owns exactly one Todo 6 Runtime. `shutdown` waits for endpoint removal and lock release,
  so command success means teardown is complete.
- Full workspace tests, strict Clippy, rustfmt, source-size review, and real terminal QA passed. LSP
  diagnostics were unavailable only because the worktree resides outside the request cwd.

## 2026-08-29 - Independent verification corrections

- Removed the temporary `ma2a-windows-security` workspace package. The exact workspace membership is
  now mechanically asserted as `ma2a-app`, `ma2a-core`, `ma2a-net`, `ma2a-runtime`, `ma2a-store`, and
  `xtask`.
- Windows caller authorization no longer trusts a peer PID or opens a peer process token. The daemon
  captures its own process-token SID, impersonates the accepted named-pipe client, reads the current
  thread token SID, and always reverts through an RAII guard.
- Unsafe Windows token FFI is confined to
  `crates/ma2a-runtime/src/ipc/windows_security.rs`; workspace tests assert that this is the only Rust
  file containing unsafe blocks or an `unsafe_code` allow.
- The shutdown replay-conflict regression now seeds replay state independently and proves that a
  syntactically valid conflicting `graceful_shutdown` request does not request server teardown.
- The 20-process daemon regression now compares the live `endpoint_info` Endpoint ID with the
  persisted repository identity while retaining the singleton lock assertion.
- Final aggregate verification passed `cargo run --locked -p xtask -- check` and 89/89 workspace
  nextest tests. Isolated CLI QA passed help, repeated status, unknown-command failure, and graceful
  shutdown with an owner-only state directory.

## 2026-08-29T15:38:23+10:00 - Independent acceptance-gate review

- Verdict: `needs-fix`. The branch is clean at `5d0022a9208fd3baaadb6b38cb51cfd1eb7137de`, exact-base ancestry and all 25 changed files were independently inspected, and all focused/aggregate gates passed, including 89/89 Rust tests, strict Clippy/rustfmt, pins/LOC, Secret Guard, and locked `xtask check`.
- Runtime QA passed autostart, repeated reuse, foreground daemon operation, unknown-command failure, owner-only Unix permissions, graceful teardown, zero/oversize/malformed/slow frames, 40-connection saturation, and post-probe availability. All review daemons, state directories, files, and tmux resources were cleaned.
- Workspace membership is exactly five product packages plus `xtask`; unsafe is confined to `crates/ma2a-runtime/src/ipc/windows_security.rs`; local `interprocess 2.4.3` source confirms protected-descriptor plumbing and default `PIPE_REJECT_REMOTE_CLIENTS`.
- The blocking gap is regression proof: `conflicting_shutdown_dispatch_does_not_request_server_shutdown` seeds replay independently and observes conflict/`false`, but calls private `dispatch` directly and never sends the conflict through a live server or proves a later request succeeds. This does not satisfy the required server-survival regression.
- Full evidence: `.omo/evidence/task-7-review/DoneClaim-AdversarialVerify.md`.

## 2026-08-29T16:10:35+10:00 - Live shutdown-conflict remediation

- Replaced the direct-dispatch-only regression with a real `LocalApiServer` and `LocalApiClient`
  scenario. Replay state is seeded independently before `serve`; the framed conflicting shutdown
  returns typed `conflict`, a subsequent framed status call succeeds, normal cancellation returns
  `ServerExit::Cancelled`, and Runtime shutdown joins both owned tasks with Endpoint closure.
- Regression sensitivity was demonstrated by temporarily restoring the old operation-based shutdown
  predicate. The new test failed when the subsequent status call received `ConnectionRefused`; after
  restoring the result-based predicate, the focused test passed.
- Final gates passed: 30/30 `ma2a-runtime` tests, strict workspace Clippy, rustfmt, 89/89 workspace
  nextest tests, locked `xtask check`, and 41/41 Web tests. RAII cleanup left no namespaced test state.
- Evidence: `.omo/evidence/task-7-review/live-shutdown-remediation.md`.
