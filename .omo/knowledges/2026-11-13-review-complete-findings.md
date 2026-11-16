# Consolidated project review — 2026-11-13

Baseline: `ce387ad`. This ledger consolidates all three review rounds and audits the separate
`2026-11-13-review-additional-findings.md` candidate note. No production fixes or commits were
made. Temporary probes were saved as patches and reversed. Earlier notes remain unchanged.

## Confirmed issue inventory

Priorities: P1 should be addressed first; P2 are actionable correctness/availability problems;
P3 are lower-impact operational/tooling defects. Evidence type is explicit: a Store or manager
probe is not presented as a complete end-to-end race. R11 is a recovery design gap; the existing
protocol explicitly documents fail-closed admission exhaustion.

| ID | Priority | Problem | Evidence | Primary location |
| --- | --- | --- | --- | --- |
| R1 | P1 | One Agent can complete another Agent's task or submit its tool calls when their generations match; ownership omits Agent ID. | Two Repository probes | `crates/ma2a-store/src/agent_output.rs:100` |
| R2 | P1 | Concurrent downloads share and truncate one partial file, allowing verified writer A to publish writer B's unverified bytes; non-replace publication is also not atomic no-clobber. | Real `Partial` API probe | `crates/ma2a-services/src/client/resume.rs:38` |
| R3 | P1 | Input backpressure hides an in-band Cancel; the cancellation timer never starts. | Real relay, paused-clock timeout | `crates/ma2a-runtime/src/hosting/call/relay.rs:127` |
| R4 | P1 | SDK accepts a terminal message that exceeds Runtime's 12,288-byte content bound, removes its task handle, and Runtime silently ignores completion. | SDK-to-Runtime decoder probe | `crates/ma2a-agent/src/sdk/task.rs:187` |
| R5 | P2 | An unconditionally compiled artifact test imports Unix-only APIs, breaking Windows all-target compilation. | Exact import fails for installed Windows target; no full native Windows build | `crates/ma2a-runtime/src/agents/artifacts.rs:218` |
| R6 | P2 | Bun check succeeds but the Vitest test entrypoint fails: Bun-specific module path and a stale credentials assertion. | 149 Bun passes versus 143 Vitest passes / 6 failures | `web/src/api/generated.test.ts:25`, `web/src/api/mutations.test.ts:33` |
| R7 | P1 | A stale callback for a removed Agent can end its recreated same-name/generation instance and new tasks. | Deterministic Store interleaving plus traced runtime path | `crates/ma2a-store/src/agents.rs:275` |
| R8 | P2 | An abandoned SSE on a silent task retains its background watch; internal channel closure never propagates from HTTP. | Real two-daemon HTTP disconnect, remote watch still active after 18 seconds | `crates/ma2a-app/src/a2a/stream.rs:167` |
| R9 | P2 | Artifact publication commits before its tool result; recovery repeats it, sees the same name, and fails with conflict. | Real Agent crash/recovery E2E | `crates/ma2a-runtime/src/agents/artifacts.rs:139` |
| R10 | P1 | Recovered exec tools replay the successful terminal but lose stdout/stderr, reporting success with empty output and `truncated: false`. | Real exec crash/recovery E2E with disclosed scheduling pause | `crates/ma2a-runtime/src/agents/tools/exec.rs:130` |
| R11 | P1 | Abandoned pending replay rows consume all 1,024 admission slots permanently; restarting does not restore fresh mutations. | Real Runtime restart plus IPC `space_create`; EOF | `crates/ma2a-store/src/mutation_replay.rs:260` |
| R12 | P2 | Failed/cancelled `service call --stdin` returns before its stdin shutdown workaround; CLI hangs until input EOF. | Real CLI cancellation with an open stdin pipe | `crates/ma2a-app/src/commands/remote/service.rs:131` |
| R13 | P2 | A retry without explicit `--version` redoes discovery and can resolve a different version, conflicting with its own RequestId. | Real CLI retry after manifest change; explicit original version still replays | `crates/ma2a-app/src/commands/remote/service.rs:116` |
| R14 | P2 | The hosted relay's stop deadline expires, but the following unbounded `lost()` write hangs again when the caller is not reading. | Real relay plus real `lost()`, paused-clock timeout | `crates/ma2a-runtime/src/hosting/call.rs:188` |
| R15 | P2 | Historical service version counters outlive manifest replacement, exceeding the browser's eight-entry bound and rejecting the whole view. | Real manager/API response with nine counters, rejected by actual Zod schema | `crates/ma2a-runtime/src/hosting/call.rs:148`, `web/src/api/services-hosted.ts:33` |
| R16 | P3 | A full backlog of withdrawn-call notices drops the operator Stop notice; the SDK treats link closure as reconnectable loss. | Real manager admission/withdrawal/stop probe; SDK consequence traced | `crates/ma2a-runtime/src/hosting/operator.rs:91` |
| R17 | P3 | Example-service manifest CLI accepts invalid max_calls values and exits successfully with an unusable manifest. | Actual binary: arguments 0 and 100 both exit 0 | `crates/ma2a-service/src/bin/ma2a-example-service.rs:159` |

R1–R6 details and verification commands: `2026-11-13-deep-project-review.md`.
R7–R10 details and verification commands: `2026-11-13-deep-project-review-round-2.md`.

## Round 3 findings and fixes to consider

### R11: replay admission can become permanently unavailable

`Reservation` has async `complete` and `release`, but no cancellation cleanup. IPC server shutdown
aborts its tasks (`ipc/server.rs:185`); a crash also leaves incomplete reservations. Retention
counts those rows but can evict only rows whose response is non-null. Pending records therefore
accumulate across interrupted runs. This is different from the intentional permanent fingerprint
fences: these particular pending rows occupy the bounded response/admission table.

The initial Store probe filled 1,024 pending rows and reopened the repository; fresh admission
failed with `local mutation replay capacity is exhausted by pending requests`. The retained probe
is stronger: reserve through a real Runtime, shut it down, start another Runtime, bind its actual
IPC server, and send a new `space_create` request. The request receives an early EOF rather than
a result. A single pending request still correctly fails closed in the existing regression test.

Fix direction: reconcile orphan reservations at authoritative daemon startup, while holding the
daemon's lifetime ownership lock, and keep the permanent request/fingerprint fences. Reclaiming
their bounded slots must not make old requests executable again. Do not indiscriminately perform
this on every `Repository::open`: other connections can open the Store while a daemon is running.
Also do not store a single generic terminal representation without distinguishing local JSON
responses from stream frames; both kinds share the table. Graceful shutdown should settle live
reservations where possible.

### R12: unsuccessful stdin calls cannot shut down their Tokio runtime

`service::call(...).await` can leave a Tokio stdin read blocked on the OS thread. The success path
explicitly exits after flushing output to avoid waiting for that uncancellable read. `called?`
returns before that path for cancellations and errors, so runtime destruction waits for stdin.

Probe: start the real example service's `hold` operation with a piped stdin kept open, wait for
the hosted call, send SIGINT, and wait until the target reports zero running calls. The CLI still
has not exited three seconds later; closing the test's stdin makes it exit with status 1. The
probe releases its owned pipe before asserting, so it does not leak a CLI on failure.

Fix direction: make stdin ownership/shutdown work on every outcome, preserving the error exit
code and flushing diagnostics. A pending blocking stdin reader must not be implicitly joined
forever during async runtime teardown.

### R13: implicit version selection changes a retried operation

The CLI resolves `newest()` before it attempts the caller-side replay lookup. An identical command
line with the same explicit RequestId can therefore produce a different canonical Open body after
the service manifest changes. The wire-level conflict check is correct; the CLI has silently
changed the operation being retried. Discovery also requires reaching the target before a cached
result can be obtained, although the retained test covers version changes rather than downtime.

Probe: execute echo using the default version 2, replace the service with a version-1-only
manifest, and replay the exact CLI arguments. The retry reports `conflict`. Repeating with explicit
`--version 2` still returns the original result, proving that the cached outcome was available.

Fix direction: durably bind resolved version to retry identity, or require an explicit version
for caller-supplied RequestIds and make that requirement clear before the first effect.

### R14: a deadline must cover final error delivery too

This is separate from R3. Here an independent service-stop token is observed, so the grace timer
does start and `relay_call` returns `Lost`. `serve` then calls `lost(writer, true)`, whose frame
write and flush have no deadline. A caller that does not drain its receive buffer blocks this
second phase forever. The service's slot is dropped before `lost`, but the inbound operation task,
transport admission permit, and operation record can remain live.

The probe connects actual frame writers/readers through duplex streams, lets service output fill
the caller buffer, stops the service, then executes the same relay-to-lost sequence as `serve`.
A virtual 60-second timeout expires. Dropping a relay write mid-frame also makes blindly appending
a new frame unsafe; that corruption risk was source-traced, not separately asserted in this probe.

Fix direction: apply one cancellation deadline to the entire operation, including final writes
and shutdown. If a frame may already be partially written or the receiver is stalled, terminate
the transport rather than attempting an unbounded terminal append.

### R15: history cardinality is not current-manifest cardinality

`Live.calls` retains counters until unregister or daemon restart. Re-registering while stopped
replaces the manifest but retains the Live entry. A manifest supports at most eight versions at
one time, but nine successive manifests can each contribute another historical counter.

The probe registers one version at a time, starts and attaches the service, admits and withdraws
a call, then stops and re-registers it. After version 9, the actual local API response contains
nine counter rows. The web projection forwards them unchanged. Feeding that projection to the
actual `HostedServicesSchema` produces `too_big` at `services[0].calls` (maximum 8). This validates
both sides of the mismatch without a fabricated API response.

Fix direction: decide whether counters belong to the current manifest or service history. Filter
or reset them if current-only; otherwise provide a bounded/paginated historical contract and align
the frontend. Simply removing the frontend limit leaves the backend's response growth unresolved.

### R16: withdrawn calls consume the supposedly reserved Stop capacity

The notice channel has capacity `MAX_SERVICE_CALLS + 1`, but Slot cleanup removes the pending call
and releases its running count without removing its already-enqueued notice. Repeated withdrawals
can therefore fill the channel even though no calls remain active. The operator's `try_send(Stop)`
then fails and its error is discarded. Dropping the sender closes the link without a Stop notice.

The probe uses the real manager's admission and the same Slot drop used by withdrawn calls,
leaving 65 notices unread, then stops the service and drains the channel. No Stop notice exists.
The full blocked-socket/SDK reconnection sequence was not executed; `ServiceHost::serve` explicitly
returns only for `LinkEnd::Stopped` and retries `LinkEnd::Lost`, establishing that consequence in
source. Priority remains P3 because service calls are stopped even though program shutdown is
misreported to the SDK.

Fix direction: make stop out-of-band/reliably signaled or remove stale notices without consuming
the reserved terminal capacity. Avoid waiting for channel capacity while holding the state mutex.

### R17: manifest generation needs the same range validation as registration

The example CLI parses `MAX_CALLS` as u8 but does not enforce 1–64 before printing. Both 0 and 100
produce exit code 0 and a manifest with that invalid value. Runtime registration rejects it later.
Use the core manifest validation or validate the documented range before emitting output.

## Additional design, contract, and documentation concerns

These are listed separately from the 17 confirmed behavioral/tooling issues. They are based on
source/document inspection; no additional end-to-end exploit or performance result is claimed.

| ID | Concern and impact | Evidence and disposition |
| --- | --- | --- |
| D1 | Service accept is coupled to unrelated local mutations. Every upgrade performs a handshake; all non-UI-lifecycle dispatches, including handshake, take the global replay mutex. A slow mutation can prevent acceptance before its 10-second deadline. | `ipc/client/stream.rs:80`, `ipc/server/dispatch.rs:119`, `hosting/call.rs:34`. The existing lifecycle tests deliberately demonstrate the blocked business plane. Decouple validation/read-only handshake from the mutation critical section; latency under this exact service scenario was not measured. |
| D2 | Hosted mutation responses do not carry their own commit receipt: register/start/stop/unregister discard the Store receipt, then read a later revision. Concurrent background writes can make that revision newer than the operation's state. | `ipc/server/execute/hosted.rs:51`, `hosting/operator.rs:34`. Source-confirmed contract inconsistency; propagate `Committed` revision. No concurrent revision probe was added. |
| D3 | Exposures survive service unregister. Reusing the name for a different implementation inherits those exposures. | `ma2a-store/src/hosted_services.rs:121` explicitly documents this behavior. This is an authorization-lifecycle choice, not a newly demonstrated authorization bypass. Make reuse consequences visible or bind exposures to registration identity. |
| D4 | Discovery and hosted calls share Service admission slots, while coarse discovery admission requires only shared-Space membership. A member without the service capability can hold pre-open slots for up to 15 seconds. | `service/catalog.rs:74`, `ma2a-net/src/service_protocol/limit.rs`, `stream_host/inbound.rs:21`. Bounds are 16 per peer / 64 globally. This is a resource-isolation hardening concern, not a cross-Space data leak; no saturation E2E was added. |
| D5 | Top-level documentation remains focused on Phase 1, while the implementation includes later services. Storage docs say schema 1; code is schema 10. The main CLI and security docs omit much of remote exec/shell/proxy and the Agent tool bridge. | `docs/storage.md:17`, `ma2a-store/src/migrations.rs:6`, `docs/cli.md`, `docs/security.md`, `docs/limitations.md`. Later protocol documents are substantially more current. Reconcile or clearly mark historical documents. |
| D6 | The service SDK depends on the Runtime crate for IPC and API types, pulling the daemon dependency graph into third-party service builds. | `ma2a-service/Cargo.toml:19`; `host.rs`, `caller.rs`, `call.rs`, and `wire.rs` imports. This is build/dependency coupling; binary-size impact was not measured. A small shared IPC/client crate would establish a cleaner boundary. |
| D7 | Permanent request/fingerprint fences grow with every admitted mutation. Streams and Agent tool mutations increase that growth rate. | Explicitly documented in `docs/protocol/local-api-v1.md:332`; migration `0006_mutation_fences.sql`. Intentional deduplication tradeoff, not accidental R11 capacity exhaustion. Define monitoring, expected growth, and any supported lifetime/epoch policy before changing retention semantics. |

Lower-priority maintenance observations from the candidate note were checked, but are not separate
functional bugs: discovery reloads authorization inputs per family; registration counts services
by loading up to 64 manifests; `ServiceAccess::Discover` has a redundant match arm after its early
return; grant-order comments omit the newer Agent/Service entries; and a fault hook named
`before_outcome_relayed` is also invoked on Reject. These warrant small cleanup or measurement,
not a blanket refactor. The manifest range problem was promoted separately to R17.

Candidate-note mapping: N1 → R11; H1 → R12; H2 → R13; H3 → R15; H4 → D1; H5 → R16;
H6 → R14; H7 → D4; H8 → D3; H9 → D2. No candidate is silently counted as dynamically reproduced.

## Verification and artifacts

- Round 1 baseline: 896 Rust tests passed; frontend check 149 passed; Vitest 143 passed / 6 failed.
  Host Bun was 1.4.0 versus repository pin 1.3.5; native Windows workspace was not run.
- Round 2: 13 existing tests passed; 4 new probes failed on the reviewed baseline.
- Round 3: 11 distinct existing tests passed across focused runs, including the ecosystem E2E
  suite, original cancellation tests, Store replay retention, and original restart fail-closed
  behavior. Six retained probes failed as intended. The final probe run was 1 existing pass /
  6 probe failures. Full-workspace baseline was not repeated after round 1.
- Additional round-3 checks: actual frontend schema rejected the real nine-counter projection;
  actual example binary successfully emitted two invalid manifests.
- All probes use temporary state, real repository/runtime/SDK paths, or deterministic duplex
  transports. Fixed IDs, strings, and requests are test inputs only, not implementation stubs.

Artifact directories:

```text
.omo/reviews/2026-11-13/
.omo/reviews/2026-11-13-round-2/
.omo/reviews/2026-11-13-round-3/
```

The third directory contains the retained patch, six-probe results, existing-test pass records,
actual counter response, frontend validation error, and manifest CLI output summary.

From the repository root, apply the round-3 probes and verify:

```bash
git apply --check .omo/reviews/2026-11-13-round-3/regression-probes.patch
git apply .omo/reviews/2026-11-13-round-3/regression-probes.patch
cargo nextest run --workspace --no-fail-fast -E 'test(review_) | test(pending_mutation_fails_closed_after_runtime_restart)'
```

On the baseline, expect the existing restart test to pass and these six probes to fail:

```text
review_restart_reclaims_abandoned_replay_capacity
review_cancelled_service_with_open_stdin_exits
review_implicit_version_retry_replays_original_call
review_stopped_call_finishes_when_caller_stops_reading
review_version_counters_remain_within_web_contract
review_stop_notice_survives_withdrawn_call_backlog
```

The full targeted existing coverage can be run with:

```bash
cargo nextest run --workspace --no-fail-fast -E 'test(review_) | binary(ecosystem_e2e) | (package(ma2a-store) & binary(mutation_replay)) | test(hosting::call::relay::tests) | test(pending_mutation_fails_closed_after_runtime_restart)'
```

Remove only the review insertions after checking that those hunks have not been edited:

```bash
git apply --reverse --check .omo/reviews/2026-11-13-round-3/regression-probes.patch
git apply --reverse .omo/reviews/2026-11-13-round-3/regression-probes.patch
```

R17 can be reproduced independently:

```bash
cargo run -p ma2a-service --bin ma2a-example-service -- manifest 0
cargo run -p ma2a-service --bin ma2a-example-service -- manifest 100
```

Current behavior: both succeed with invalid max_calls. Corrected behavior should reject both with
a nonzero status and a 1–64 range diagnostic. Valid boundary values 1 and 64 should still work.

## Important exclusions

- Agents run as the same OS user and are not a sandbox. R1 is an ownership/configuration failure,
  not a demonstrated cross-OS-user escalation.
- Process retention already archives old ended local instances before retrying; no seventeenth-
  restart permanent failure was established.
- Large local Agent message views already implement elision; no generic missing size guard was
  established there.
- Proxy listeners being non-durable is documented behavior.
- Unbounded permanent fingerprint storage is an explicit tradeoff (D7), distinct from R11.
- No speculative service-registration race was promoted without a sufficiently verified failure
  path. Findings here do not imply exhaustive coverage of every platform or interleaving.
