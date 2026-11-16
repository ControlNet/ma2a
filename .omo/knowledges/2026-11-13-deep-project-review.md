# Deep project review — 2026-11-13

Reviewed revision: `ce387ad`. The working tree was clean before review. Production code and existing tests are unchanged after review; the review artifacts contain a replayable test patch and sanitized evidence. No fixes or commits were made.

## Scope and method

Source tracing across the Rust workspace and frontend, with particular attention to Agent ownership and SDK completion, hosted service cancellation, transfer integrity, durable replay, supervisor lifecycle, authorization withdrawal, A2A streaming, and CI portability. Existing knowledge notes and protocol documents were consulted to rule out already handled or intentional behavior. This is a targeted deep audit, not a claim that every execution interleaving or platform was tested.

The five focused probes exercise real repository APIs, the real SDK and Runtime decoder, the real hosted relay over in-memory streams, and the real download staging implementation. Fixed identifiers and byte content are synthetic test inputs only. They create temporary state and never use a running user's daemon or real credentials.

## Findings

### R1 — P1: Agent output can mutate another Agent's task

Location: `crates/ma2a-store/src/agent_output.rs:100` (`held`).

`held` checks task generation and running/waiting state but never compares `record.summary.agent` with `output.agent`. Generations are scoped to an Agent: two first instances both have generation 1. The outer output-cursor check only establishes that the emitting Agent's instance is live. It does not establish ownership of the referenced task.

Reproduced with two real stored Agent definitions and a running task of the second Agent: applying the first Agent's completed report changes the second task to completed. A separate probe registers a tool call for the second task. Both should be ignored but are accepted. Runtime `agents/tools.rs` derives the executing Agent and tool allowlist from the task record, so this also creates a path to using the wrong Agent's tool configuration when the configurations differ. This is an internal workload isolation/correctness failure; Agent programs are explicitly not OS-sandboxed.

Fix direction: require `(agent_id, generation)` ownership in every task-affecting output branch; test cross-Agent messages, progress, terminal reports, and tool calls with equal generations.

Evidence: `../reviews/2026-11-13/store-probes.txt`.

### R2 — P1: Concurrent downloads can publish unverified bytes

Locations: `crates/ma2a-services/src/client/resume.rs:38` and `:108`; CLI caller: `crates/ma2a-app/src/commands/remote/transfer.rs:28`.

Every download to a destination uses the same `.ma2a-part` and sidecar names. `Partial::open` uses truncating `File::create` without exclusive ownership. A second download can truncate or modify the inode that the first still holds. Verification hashes received bytes in memory; `finish` subsequently renames the shared path and does not establish that it still contains those verified bytes.

Deterministic reproduction: first writer writes its content and updates its hash; second writer opens the same destination and writes different content; first writer successfully finishes. The final destination contains the second writer's unverified bytes. The publication path also checks destination existence separately from rename; that is not an atomic no-clobber operation when `replace` is false.

Fix direction: acquire an exclusive per-destination download lease before touching partial/sidecar state, retain ownership through verification/publication/cleanup, and use an atomic no-clobber publication primitive for non-forced downloads. Resume needs the same ownership rule.

Evidence: `../reviews/2026-11-13/download-probe.txt`.

### R3 — P1: Backpressure still hides caller cancellation indefinitely

Location: `crates/ma2a-runtime/src/hosting/call/relay.rs:127`; related generic relay: `crates/ma2a-runtime/src/stream_plane/relay.rs` (`upstream`).

The hosted relay pump awaits forwarding a complete frame into a bounded pipe before reading the next caller frame. If the service does not read, both transport and the 128 KiB pump pipe fill. A Cancel frame behind buffered data is then never read, so `ending.cancelling` is never set and `STUCK_GRACE` never starts. The generic caller-side relay has the same ordering issue. The existing regression covers operator stop while flooding and caller cancel while the service drains input; it does not cover cancel behind blocked input.

Reproduced using 512 KiB of queued data followed by Cancel, with the service connection held open but unread. The paused-clock 60-second timeout expires. The operation and its capacity can remain occupied until an independent stop or connection failure.

Fix direction: make transport disconnect/cancellation observable independently of a blocked data forward, and define a bounded stall policy for paths where an in-band Cancel cannot overtake data. Merely increasing a buffer postpones the problem. Apply the policy to both generic and hosted relays.

Evidence: `../reviews/2026-11-13/runtime-probes.txt`.

### R4 — P1: SDK completion silently disappears at the message bound

Locations: `crates/ma2a-agent/src/sdk/task.rs:187`, `crates/ma2a-agent/src/sdk.rs:64`, `crates/ma2a-runtime/src/agents/lines.rs:117`.

SDK `Task::complete` treats enqueueing a line under 64 KiB as success, removes the task slot, and disables its Drop fallback. Runtime separately requires message content at most 12,288 bytes. For an ASCII result of 12,289 bytes, the SDK emits completed successfully but Runtime converts the whole state report to Ignore. The task remains running or waiting while its SDK handle no longer exists. If the Agent stays alive, ordinary instance maintenance does not repair the task; it requires cancellation or instance termination.

Reproduced by running the real SDK over duplex streams, completing a valid task with 12,289 bytes, and passing its terminal line to the actual Runtime decoder: the decoded effect is Ignore.

Fix direction: validate message semantics in the SDK before releasing the task; either report a small valid failure or return a recoverable API error. Apply the same checks to `ask`, multipart messages, and invalid NUL content. Prefer a shared message validation contract to two diverging bounds.

Evidence: `../reviews/2026-11-13/runtime-probes.txt`.

### R5 — P2: Windows all-target checks cannot compile the artifact test

Location: `crates/ma2a-runtime/src/agents/artifacts.rs:218`.

The test module is guarded only with `cfg(test)`, but imports `std::os::unix::fs::MetadataExt` and calls Unix-only `ino`, `mtime_nsec`, and `mtime`. `agents::artifacts` itself is not Unix-gated. Native CI includes Windows and executes `cargo check --locked --workspace --all-targets --all-features`, which compiles this module.

The exact import was independently compiled for the installed `aarch64-pc-windows-msvc` standard library and failed with E0433. This verifies the platform API mismatch; a full native Windows workspace build was not executed here.

Fix direction: gate the Unix-specific assertions/test or provide an equivalent Windows file-identity check. Retain a portable modified-time/content check where possible.

Evidence: `../reviews/2026-11-13/windows-import.txt`.

### R6 — P2: Frontend test entrypoints disagree and the advertised test command fails

Locations: `web/package.json:12`, `web/src/api/generated.test.ts:25`, `web/src/api/mutations.test.ts:33`.

`bun run check` executes Biome, TypeScript, and Bun's native test runner. `bun run test` and coverage execute Vitest. The generated-contract tests use Bun-specific `import.meta.dir`, which is undefined under the Vitest execution here. Five tests fail with paths under `undefined/`. A sixth test expects credentials `include` although the implementation explicitly selects `same-origin`.

Observed: `bun run check` succeeds with 149 tests; `bun run test` reports 143 passed and 6 failed. This is a test portability/verification issue, not evidence that the same-origin cookie policy is wrong. The review host has Bun 1.4.0; the repository pins 1.3.5. The import portability and explicit assertion mismatch are visible in source; pinned Bun/native CI was not substituted by this local run.

Fix direction: choose one supported test runner or make the suite portable across both; use URL-based module paths and align the cookie assertion with the intended same-origin policy.

## Reviewed non-findings

- Agent restart retention: raw Store creation refuses a seventeenth retained Process, but `ProcessManager::start_local` archives the oldest ended local Process and retries. A raw Store-only probe cannot establish a restart bug; it was excluded from the retained patch.
- Large JSON Agent message views: `api/agent_views.rs` already elides oversized serialized messages and flags the elision. A generic JSON expansion concern is not a missing response-size guard here.
- Proxy tool listeners are explicitly documented as non-durable and are not recreated after daemon restart.
- Same-user Agent programs are not a security sandbox; findings should not be described as a new cross-OS-user privilege escalation.

## Reproduction

Run from the repository root at the reviewed revision. The patch adds only tests, including insertions into two existing test modules. It should be applied only to a clean or compatible checkout; check application first.

```sh
git apply --check .omo/reviews/2026-11-13/regression-probes.patch
git apply .omo/reviews/2026-11-13/regression-probes.patch
cargo test -p ma2a-store --test review_regressions -- --nocapture
cargo test -p ma2a-runtime --lib review_ -- --nocapture
cargo test -p ma2a-services --test review_download -- --nocapture
```

Expected before fixes: two Store assertion failures, two Runtime assertion failures, and one download assertion failure. After fixes all five should pass. The Cancel test uses Tokio virtual time; its 60-second bound need not consume 60 seconds of wall time.

The following command removes only the probe patch. It reverses the two test insertions and deletes the two test files created by the patch; do not run it after editing those probe files without checking the reverse first.

```sh
git apply --reverse --check .omo/reviews/2026-11-13/regression-probes.patch
git apply --reverse .omo/reviews/2026-11-13/regression-probes.patch
```

Baseline commands:

```sh
cargo nextest run --workspace --no-fail-fast
(cd web && bun run check)
(cd web && bun run test)
```

Native Windows verification after fixing R5:

```sh
cargo check --locked --workspace --all-targets --all-features
```

Expected: the native Windows command compiles all tests without Unix-only imports; both supported frontend test entrypoints should agree after R6 is corrected.

## Final verification results

- Existing Rust workspace suite: **896 passed, 0 failed, 0 skipped**, one slow test, 407.849 seconds of execution after compilation.
- Frontend `bun run check`: Biome and TypeScript passed; **149 tests passed**.
- Frontend `bun run test`: **143 passed, 6 failed** under Vitest.
- Focused regression probes: **5 expected assertion failures** confirming R1–R4. These were run separately from the baseline suite.
- Windows exact-import compiler probe: **E0433**, as expected for R5.
- Probe patch reversed successfully; tracked source files match the reviewed baseline.
- Concise baseline evidence: `../reviews/2026-11-13/baseline-results.txt`.
