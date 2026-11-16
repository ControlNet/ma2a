# Deep project review, round 2 — 2026-11-13

Reviewed baseline: `ce387ad`. This continues the first review; findings R1–R6 remain in
`2026-11-13-deep-project-review.md`. No production fixes were applied in either review.

Four additional findings were reproduced. The temporary tests and scheduling hook were saved in
`.omo/reviews/2026-11-13-round-2/regression-probes.patch`, then reversed. The final tracked tree
matches the baseline. Evidence is in `.omo/reviews/2026-11-13-round-2/verification.txt`.

## R7 — P1: a removed Agent's instance can conclude a recreated Agent's tasks

Locations:

- `crates/ma2a-store/src/agents.rs:110`: a newly inserted definition starts at generation zero.
- `crates/ma2a-store/src/agents.rs:145`: undefine deletes that definition and its instance.
- `crates/ma2a-store/src/agents.rs:275`: conclusion checks only Agent name and generation.
- `crates/ma2a-runtime/src/agents/operator.rs:111`: stop disables/concludes the durable instance
  before awaiting termination of its Process.
- `crates/ma2a-runtime/src/agents/link.rs:58`: the outstanding follower can conclude its captured
  instance after the process ends.
- `crates/ma2a-runtime/src/agents/instance.rs:124`: conclusion forwards only name and generation.

An overlapping stop / undefine / define / start can recreate the same name with generation 1
while the old generation-1 follower is still alive. Its later conclusion matches the new row,
ends the new instance, clears its outbox, and fails its assigned tasks as `agent_lost`.
The link registry also keys its identity by name and generation, so the collision is not confined
to one SQL statement. The definition creation timestamp already used for process start RequestIds
does not protect these paths.

Evidence: `review_old_instance_cannot_end_recreated_agent` replays this interleaving through the
real Repository API, using distinct definition creation timestamps. Both generations are 1;
the stale conclusion fails one new task, changing `Running` to `Failed`. This is a deterministic
Store reproduction plus a traced runtime path, not a fully scheduled end-to-end lifecycle race.

Fix direction: persist an incarnation identifier or a generation counter that survives deletion,
and include it in link coordination, output application, process binding, and conclusion. Merely
serializing the operator commands does not invalidate asynchronous callbacks already in flight.

## R8 — P2: disconnected A2A SSE clients leave silent remote watches open

Locations:

- `crates/ma2a-app/src/a2a/stream.rs:100`: response production runs in a detached task.
- `crates/ma2a-app/src/a2a/stream.rs:167`: the producer selects only internal events and follower
  completion; it does not select `frames.closed()`.
- `crates/ma2a-app/src/a2a/remote.rs:139`: the remote follower notices closure of its internal
  event receiver, which the leaked producer continues to own.
- `crates/ma2a-core/src/service/catalog.rs:95`: Agent streams have per-peer/global quotas of
  32/128, shared with other Agent operations.

For an input-required task with no new events, dropping the HTTP connection never reaches the
next frame send, so the producer cannot discover that its consumer is gone. Remote watches stay
open; the local route instead keeps polling. Repeated authorized subscriptions and disconnects
can consume the Agent stream quota and interfere with unrelated Agent calls from that Endpoint.

Evidence: `a2a::review_disconnected_sse_releases_remote_watch` starts two real daemons and the
actual adapter, submits `ask colour?`, subscribes, waits for the initial SSE and a live remote
watch, then shuts down the TCP connection. After 18 seconds, the target's `ops --json` still shows
one active `agent_watch`. Expected count is zero. No simulated transport or backend was used.

Fix direction: bind producer lifetime to response receiver lifetime, including initial backend
reads. For example, select the whole producer against the frame sender's closure and drop the
follower promptly. Keep task cancellation separate: disconnecting a subscriber should release
the subscription, not cancel the underlying durable task.

## R9 — P2: artifact publication succeeds, then fails on crash recovery

Locations:

- `crates/ma2a-runtime/src/agents/artifacts.rs:139`: publication commits the artifact independently
  of the tool result.
- `crates/ma2a-runtime/src/agents/tools.rs:113`: the tool result commits only after execution.
- `crates/ma2a-store/src/agent_actions.rs:202`: any already-taken artifact name is refused.
- `crates/ma2a-runtime/src/agents/maintain.rs:64`: pending tool calls execute again after restart.

If the daemon crashes after `add_agent_artifact` commits but before `complete_tool_call` commits,
recovery repeats `artifact.publish`. The same task/name now conflicts, so the original successful
publication becomes a tool error. With the supplied example Agent, the whole task fails, even
though its artifact is present. The existing crash-recovery test performs `transfer.get` first,
so its generic `before_tool_result` fault does not reach this publication-specific window.

Evidence: `recovery::review_artifact_publish_recovers_after_effect_commit` uses a real workspace
file containing test text, the reference Agent, and the existing `before_tool_result` crash hook.
After recovery, the task has exactly one artifact but is `failed` with a `conflict` error instead
of `completed`. No extra scheduling hook is needed for this test.

Fix direction: make the publication effect and its stored tool outcome one transaction, or bind
publication to the originating task/call and replay that result. Do not simply accept every
existing name: a genuinely different publication under that name must still conflict.

## R10 — P1: recovered exec tools silently report empty output as successful

Locations:

- `crates/ma2a-runtime/src/agents/tools/exec.rs:130`: each execution constructs empty captures.
- `crates/ma2a-services/src/client/process.rs:66`: a replayed terminal returns without reading
  stdout/stderr data frames.
- `crates/ma2a-runtime/src/agents/tools/exec.rs:149`: those empty captures become a successful
  `ExecOutput`, with `truncated` false.
- `crates/ma2a-runtime/src/ipc/server/stream.rs:214`: mutation replay persists the terminal frame,
  not the streamed output.

When an exec's successful terminal has been journaled but its tool result has not, restarting the
daemon replays the terminal under the derived RequestId. The command is not rerun, which correctly
avoids duplicate effects, but its original stdout/stderr are not recovered. The bridge presents
the missing output as genuinely empty output with exit code 0 and no truncation warning. An Agent
can therefore make subsequent decisions using a false successful tool result.

Evidence: `recovery::review_exec_output_survives_tool_result_recovery` first verifies that the real
remote command `printf durable-output` returns that output. It then crashes at the existing tool
result fault point and recovers. Observed result:

```json
{"exit_code":0,"reason":"completed","stderr":"","stdout":"","truncated":false}
```

This test adds a **review-only 500 ms scheduling pause** before the existing crash point, enabled
only by `MA2A_TEST_REVIEW_SETTLE_REPLAY`. Its purpose is to let the independently committed replay
terminal reach storage before the crash. It does not replace the service, fabricate a result, or
change replay behavior. The hook is included in the saved patch and was removed from the working
tree. Without the pause, the earlier crash window was also observed: recovery returns
`outcome_unknown`, which is an explicit failure and is not this silent-output-loss finding.

Fix direction: persist captured output as part of the recoverable tool operation, or expose the
fact that only a terminal was replayed and return an explicit output-unavailable error. Never
re-execute arbitrary commands solely to reconstruct output, because they can have side effects.

## Verification and reproduction

The final focused run executed 17 tests: **13 existing tests passed, 4 review probes failed**.
The existing tests include the Store Agent lifecycle suite, the full A2A client E2E test, and the
existing `before_tool_result` recovery E2E test. The complete 896-test suite was run in round 1;
it was not rerun for this review-only round. Test daemons and Agents use temporary state and the
repository's deterministic reference implementation; no language model or production state is
involved. The artifact test's text file is test input only.

From the repository root, apply the preserved probes and run the same verification:

```bash
git apply --check .omo/reviews/2026-11-13-round-2/regression-probes.patch
git apply .omo/reviews/2026-11-13-round-2/regression-probes.patch
cargo nextest run --workspace --no-fail-fast -E 'test(review_) | test(a2a_clients_use_local_and_remote_agents) | test(a_crash_before_a_tool_result_is_stored_answers_it_once) | (package(ma2a-store) & binary(agents))'
```

Expected on the reviewed baseline: 13 pass and these four failures:

```text
review_old_instance_cannot_end_recreated_agent
a2a::review_disconnected_sse_releases_remote_watch
recovery::review_artifact_publish_recovers_after_effect_commit
recovery::review_exec_output_survives_tool_result_recovery
```

The following removes only the applied review patch; check first and do not run it if those hunks
have since been edited for other work:

```bash
git apply --reverse --check .omo/reviews/2026-11-13-round-2/regression-probes.patch
git apply --reverse .omo/reviews/2026-11-13-round-2/regression-probes.patch
```

The recurring design issues are incomplete incarnation identity, detached stream lifetimes, and
confusing mutation deduplication with recovery of the tool's full result. Recovery tests need to
assert result content and per-tool semantics, not only that an operation avoids a second effect.
