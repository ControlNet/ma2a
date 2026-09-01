# Todo 21 Phase One E2E

- The exact `ma2a-app` E2E target can compose accepted control-sync fixtures and real local Iroh relay servers without adding production transport seams.
- Scenario A must compare the target lookup result against both the target's signed relay and the unrelated Space-advertised relay before dialing.
- Scenario B observes both the supplied fallback candidate and Iroh's connected public home; scenario C authenticates two independently authorized peers through their common private relay; scenario D is a deterministic empty-map `DegradedNoCommonHome` assertion.
- Scenario E is restart-based: a new Endpoint instance with the same identity must connect through freshly supplied target data, so no transient learned route is required.
- Scenario F changes the live Iroh relay map, waits on `home_relay_status()` without sleeps, publishes the changed `EndpointAddr` without `force_advance`, preserves Endpoint identity, and advances the signed address sequence exactly once.
- Reusing the live authenticated forwarding responder under the exact E2E target provides durable before/after equality for manifest fork, address rollback, expired relay, signer mismatch, and cross-Space injection.
- The exact release target composes enrollment, authorization isolation, control synchronization, propagated revocation, Echo, IPC, stale-daemon recovery, Web security/session mutation, relay TLS, and relay scenarios A-F.
- Three parallel nextest repetitions each passed 35/35 and emitted nine structured records: A-F plus signer-claim rejection, persistent identity restart, and control-sync high-water evidence.
- Per-run JUnit reports record `tests=35`, `failures=0`, `errors=0`, and `skipped=0`; raw logs are colorless and retained for diagnosis.

## Final Oracle Rejection Resolution

- Final committed HEAD is `e59a90c857c165b097e73d587c9da85154bcd3f3` on `todo-21-phase-one-e2e`.
- Scenario B supplies the filtered Public-only relay map to Iroh and records Iroh's connected effective home; Scenario C runs the production private-relay admission/server path for peers authorized through either served Space; Scenario D derives exact `DegradedNoCommonHome` from a production Runtime.
- Scenario E first establishes a transient route, closes the Endpoint, restarts with the same identity, and reconnects from fresh signed target data. Scenario F changes the live observed home, advances address records from sequence 2 to 3, restarts the source Runtime, actively synchronizes with no pre-existing connection, and verifies exact durable sequence/address receipt in both shared Spaces.
- The exact release target now composes 42 tests, including enrollment race/replay and process-level daemon singleton, stale-socket, permissions, shutdown, and version-mismatch coverage.
- The adversarial forwarding responder no longer reclaims a previously persisted global port. It binds an ephemeral port and publishes that live address into its isolated candidate fixture, eliminating cross-test port collisions under parallel Nextest execution.
- `scripts/run-e2e.sh` rejects zero/nonnumeric repetitions, clears stale output, requires `jq`, validates exact JUnit totals (`42/42`, zero skipped/failures/errors), validates exactly nine unique structured evidence records and scenario-specific schemas, records commit/toolchain provenance, and writes SHA-256 manifests.
- Three final repetitions passed 42/42 with nine validated evidence records each. Rustfmt, strict Clippy (`-D warnings`), checksum verification, tracked/staged Secret Guard scans, and the 250 pure-LOC ceiling all passed.
- Final sanitized evidence archive: `/home/zhixi/GitRepos/ma2a/.omo/evidence/task-21-ma2a-phase-0-1.zip`.

## 2026-08-31 Fresh Oracle Acceptance Re-audit

VERDICT: REJECT

- Corrected HEAD `e59a90c857c165b097e73d587c9da85154bcd3f3` passes the exact target independently at 42/42, and the repeated runner passes three runs with 42 tests and nine validated JSONL records per run. `MA2A_E2E_RUNS=0` exits 2; rustfmt, strict target Clippy, LOC, tracked Secret Guard, and archive SHA-256 checks pass. LSP remains unavailable because the isolated `/tmp` worktree is outside the request cwd.
- Prior scenario defects B-F are behaviorally closed: B supplies `RelayMatrix::map(...).relay_map()` to Iroh and observes the supplied Public home; C derives real `PrivateRelayAccess` from both Space authorization views and proves both real relay paths; D reads exact `DegradedNoCommonHome` from production Runtime status; E establishes a real pre-restart route then creates a fresh Endpoint with the same identity and fresh signed lookup data; F observes a live home change, requires sequence `+1` in both Spaces, actively control-syncs with no prior observation, and validates durable signed addresses.
- Blocking exact-target composition gap: `crates/ma2a-app/tests/e2e.rs` does not compose the required slow IPC frame test (`ma2a-runtime/src/ipc/framing.rs`), forced process-kill transaction recovery (`ma2a-store/tests/transactions.rs`), SSE overflow/resnapshot and dropped-receiver cases (`ma2a-runtime/src/web/runtime_routes_tests.rs` / `ma2a-runtime/tests/web_runtime.rs`), or a relay-outage case. These tests existing in other binaries cannot satisfy Todo 21s exact `--test e2e` gate.
- Blocking persistent high-water gap: `restart_preserves_endpoint_identity_and_control_high_water` asserts exact Endpoint ID but only `revision_after >= revision_before`; it neither records nor compares manifest, address, and relay high-water state across restart, so regression-sensitive control high-water preservation is not proved.
- Evidence archive `/home/zhixi/GitRepos/ma2a/.omo/evidence/task-21-ma2a-phase-0-1.zip` exists, has provenance for `e59a90c857c165b097e73d587c9da85154bcd3f3`, contains three 42/42 JUnit reports and 27 total scenario records, exposes no detected credential/token material, and its per-run digests verify.

## 2026-08-31 Exact-Target Correction

- Corrected final HEAD is `578260d3306f549891074854cefba3312439b1cc`, preserving rejected HEAD `e59a90c857c165b097e73d587c9da85154bcd3f3` and adding three focused commits.
- The exact `ma2a-app --test e2e` target now composes 46 tests. Added scenarios prove bounded partial IPC rejection with unrelated-client progress, forced process-kill Store all-or-none recovery plus SQLite integrity, SSE overflow resync plus dropped-receiver permit release, and local relay outage replacement without public Internet.
- Persistent restart evidence now actively synchronizes control state and compares exact manifest generation/hash, address sequence/hash, and relay sequence/hash tuples for both shared Spaces before and after restart. All values must be non-default and exactly equal.
- `scripts/run-e2e.sh` requires exactly 46 JUnit cases and 13 unique structured scenario records per repetition, validates the new schemas, verifies each SHA-256 manifest immediately, and checks exact repetition artifact counts.
- Three consecutive final repetitions passed 46/46 with 13 records each. Provenance names `578260d3306f549891074854cefba3312439b1cc`; all nine per-run artifact digests verify.
- Final evidence archive is `/home/zhixi/GitRepos/ma2a/.omo/evidence/task-21-ma2a-phase-0-1.zip`, SHA-256 `2f5e9dffd04cf447618baa88ab8029dd9210ac6ba2b259d311d79370433271d4`. ZIP integrity and Secret Guard scans of tracked source, staged commit sets, raw evidence, and the evidence directory passed.
- One earlier third repetition observed the pre-existing Scenario A test emit correct evidence and then fail with Iroh `internal consistency error` during teardown. A clean full three-repetition rerun passed without source changes; retain this as a transient relay teardown flake candidate.
- Rust LSP diagnostics remained unavailable because the delegated request cwd excludes the isolated `/tmp` worktree. Rustfmt, strict Clippy, exact nextest, LOC, locked aggregate checks, and direct evidence validation passed as fallback diagnostics.
