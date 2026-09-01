# Todo 10 findings

## 2026-08-30 implementation

- Endpoint-centric authorization must evaluate one complete Space at a time. An `any` over complete per-Space decisions preserves independent authorities and prevents cross-Space capability composition.
- The request authority source is the authenticated caller Endpoint plus local target Endpoint. A Space cursor is a typed resource selector only after shared membership is established.
- Uniform denial is easiest to preserve when the engine exposes one denial value and keeps the allowing Space, generation, and manifest hash inside an opaque redacted permit.
- Passing current verified Space views into the runtime entry point on every request avoids stale authorization caches and makes committed revocations effective on the next call.
- The normal ALPN inventory should remain closed: classify known ALPNs into typed operations and return no operation for unknown protocols.
- Zero-Space isolation is strongest at transport negotiation. Registering only enrollment prevents malformed or oversized normal-service bodies from reaching stream or parser code.
- Real-Iroh regression tests should attempt both malformed and oversized payloads against every normal ALPN and an unknown future ALPN, then prove enrollment remains independently reachable.
- Production currently has exactly one protocol handler and router registration, both enrollment-only; future normal handlers must authorize through the runtime seam before reading bodies.
- Isolated `/tmp/opencode` worktrees cannot currently be inspected by the LSP tool because it requires paths under the request cwd. Use compiler, strict Clippy, rustfmt, focused tests, full nextest, and `xtask check` as explicit fallback evidence.

## 2026-08-30T08:14:50+10:00 - Independent acceptance review

- Verdict: REJECT. The implementation source is fail-closed and the required command suite passed, but the plan's required exhaustive membership/policy/revocation matrix is incomplete.
- Blocking finding: `crates/ma2a-core/tests/authorization_matrix.rs:182-202` property-tests only `RemoteOperation::ECHO_CALL`, and `crates/ma2a-core/tests/authorization_matrix.rs:238` fixes every generated Space to `SpacePolicyV1::phase_one_default()`. The distinct relay-provider policy/member-capability branch in `crates/ma2a-core/src/authorization.rs:78-80` has no positive, member-denial, or policy-denial test, while `CONTROL_SYNC`, `METADATA_READ`, and `ADDRESS_RECORD_EXCHANGE` have no complete allow/deny matrix. This does not satisfy plan lines 182 and 187 requiring table-driven/property coverage over membership, policy, and revocation combinations and exhaustive matrices.
- Source trace otherwise passed: each Space is evaluated independently with `.find(space.permits)`; partial Echo grants do not compose; a revoked Space does not cancel another complete allow; unsupported operations deny; control cursors narrow only a currently shared Space; denial has one value/message; `AuthorizationPermit` fields are private, has redacted `Debug`, no serde implementation/dependency, and cannot be externally serialized under Rust orphan rules.
- Registration trace passed: `RuntimeEndpoint::bind_with_lookup` advertises and registers only `ENROLLMENT_ALPN`; `RuntimeEndpoint` keeps its `Router` private; the only production `ProtocolHandler` is enrollment. No normal handler or normal body parser exists, so current normal protocols cannot bypass `authorize_remote`; all five known normal ALPNs and one unknown future ALPN are rejected during real Iroh negotiation before stream/body access.
- Reproduced passes: authorization matrix 8/8; zero-Space real-Iroh 2/2 twice; Core/Net/Runtime/App package suite 175/175; rustfmt; workspace all-target/all-feature Clippy with `-D warnings`; LOC; locked aggregate check with 213/213 Rust and 44/44 Web tests; diff check; Secret Guard staged/tracked/gitignore; no tracked/staged worktree drift.
- LSP diagnostics were attempted for all eight changed Rust files and rejected each isolated `/tmp/opencode` path as outside the request cwd. Compiler, Clippy, rustfmt, nextest, and aggregate gates are the fallback evidence.
- Cleanup passed: no `ma2a` process and no `/tmp/ma2a-zero-space-isolation-*` artifact remained.
- Reproduce the blocker inventory: `cd /tmp/opencode/ma2a-todo-10-authz-20260830 && rg -n "RemoteOperation::(ECHO_CALL|CONTROL_SYNC|METADATA_READ|ADDRESS_RECORD_EXCHANGE|RELAY_ADVERTISEMENT)|SpacePolicyV1::phase_one_default" crates/ma2a-core/tests/authorization_matrix.rs`.
- Reproduce the focused suite: `cd /tmp/opencode/ma2a-todo-10-authz-20260830 && cargo nextest run --locked -p ma2a-core --test authorization_matrix`.

## 2026-08-30T08:50:27+10:00 - Acceptance matrix remediation

- Replaced the Echo-only matrix with one integration-test root and four coherent test-only modules for explicit signed-Space fixtures, supported operations, relay authorization, and resource/default-denial behavior.
- The fixture now takes explicit Echo and relay policy grants and re-signs canonical genesis objects, so generated Spaces exercise enabled and disabled policies instead of inheriting `SpacePolicyV1::phase_one_default()` as the test oracle.
- Exhaustive tables cover `ECHO_CALL`, `CONTROL_SYNC`, `METADATA_READ`, `ADDRESS_RECORD_EXCHANGE`, and `RELAY_ADVERTISEMENT` across zero, one, and many shared Spaces; they prove any complete independent Space allows, revocation remains Space-local, and zero/one/many denials are externally equal.
- Relay-specific cases prove positive allow, missing caller member capability denial, policy denial, caller revocation denial, independent-other-Space allow, and non-composition of policy and member grants from separate Spaces.
- The expanded property selects all five supported operations and derives its expected result only from generated complete/denied Space outcomes (`first_complete || second_complete`), rather than reproducing the production `permits` match algorithm.
- Mutation proof 1: temporarily replacing the relay capability/policy predicate with `true` made three tests fail: missing member capability, policy exclusion, and cross-Space relay composition. The mutation was reverted before verification.
- Mutation proof 2: temporarily denying `METADATA_READ` made three tests fail: supported-operation cardinality, revocation locality, and the expanded any-complete-Space property. The mutation and generated proptest regression artifact were reverted before verification.
- Focused post-commit verification: `cargo nextest run --locked -p ma2a-core --test authorization_matrix` passed 16/16 tests; rustfmt check passed.
- Strict workspace Clippy with all targets/features and `-D warnings` passed; `cargo run --locked -p xtask -- check-loc` passed; Rust no-excuse checks passed for all five changed test files.
- Locked aggregate verification passed 221/221 Rust tests and 44/44 Web tests, with advisories, bans, licenses, sources, formatting, unused-dependency analysis, TypeScript, and build gates successful.
- Pure LOC: root 4, fixtures 211, operations 245, relay 71, resources 64. `operations.rs` is in the warning band and should be split before future growth.
- LSP diagnostics were attempted for every changed test file and remained unavailable because the isolated `/tmp/opencode` worktree is outside the request cwd; compiler, Clippy, rustfmt, nextest, LOC, and aggregate gates provide fallback evidence.
- Atomic remediation commit: `dd69c17a72fee188b8c61f011b8fdb516a35a6a8` (`test(core): exhaust authorization acceptance matrix`). Final product worktree is clean; `.omo/**` remains uncommitted; no push was performed.

## 2026-08-30T09:35:17+10:00 - Independent remediation review

- Verdict: REJECT. Production authorization remains fail-closed and every required command passes, but the remediation still does not independently prove Echo policy exclusion as required by the exhaustive membership/policy/revocation matrix criterion.
- Blocking finding: `crates/ma2a-core/tests/authorization_matrix/operations.rs:33-37` defines `ECHO_POLICY_DENIED`, but every denial-only use combines it with caller/target revocation (`operations.rs:93-95`, `operations.rs:217-224`, and `operations.rs:291-295`). Its only unrevoked use is paired with an independent complete allow (`operations.rs:120-127`). Therefore no test supplies one otherwise-complete, non-revoked Echo Space with policy disabled and expects `ACCESS_DENIED`.
- Mutation proof: temporarily changed `ECHO_POLICY_DENIED` from `PolicyGrants::NONE` to `PolicyGrants::ECHO`; `cargo nextest run --locked -p ma2a-core --test authorization_matrix` still passed 16/16. This proves the suite cannot detect removal of the Echo policy-exclusion case. The mutation was reverted, the original suite again passed 16/16, and the worktree returned clean at exact HEAD `dd69c17a72fee188b8c61f011b8fdb516a35a6a8`.
- Required fix: add an isolated Echo case whose sole current shared Space grants both Endpoint Echo member capabilities, disables Echo in Space policy, has no revocation, and must return `AuthorizationDenied::ACCESS_DENIED`. Keep the existing relay-specific policy exclusion test.
- Other acceptance evidence passed: all five operations are present in `OPERATION_CASES`; zero/one/many, any-one-complete-Space allow, non-composition, Space-local revocation, cursor narrowing, unsupported default denial, denial equality, and relay member/policy branches are covered. Production `authorize_endpoint` evaluates complete Spaces independently, and production protocol registration remains enrollment-only.
- Reproduced gates: Core authorization matrix 16/16; real-Iroh zero-Space isolation 2/2; locked aggregate `xtask check`; base-to-HEAD `git diff --check`; Secret Guard staged/tracked scans; no TODO/FIXME/HACK/conflict markers; no running product process or `/tmp/ma2a-zero-space-isolation-*` artifact.
- LSP diagnostics were attempted for all 12 changed Rust files and rejected each isolated `/tmp/opencode` path because it is outside the request cwd. Compiler, test, aggregate, and diff gates are the fallback evidence.

## 2026-08-30T10:01:03+10:00 - Echo policy-exclusion remediation

- Added `crates/ma2a-core/tests/authorization_matrix/echo.rs` with one isolated Echo denial case: one shared Space, caller and target both Echo-capable, Echo excluded by policy, and no revocation. The exact result is `AuthorizationDenied::ACCESS_DENIED`.
- Kept `operations.rs` unchanged at 245 pure LOC by registering the coherent Echo-specific test module from `authorization_matrix.rs`. The root is 10 pure LOC and the new module is 24 pure LOC.
- Mutation proof: temporarily changed the new fixture policy from `PolicyGrants::NONE` to `PolicyGrants::ECHO`; the isolated test failed because authorization succeeded (`left: None`, `right: Some(AuthorizationDenied)`). The mutation was reverted before verification.
- Focused verification passed: authorization matrix 17/17 and real-Iroh zero-Space isolation 2/2.
- Full verification passed: rustfmt, strict workspace Clippy with all targets/features and `-D warnings`, LOC, Rust no-excuse checks, aggregate `xtask check` with 222/222 Rust and 44/44 Web tests, base-to-HEAD diff check, marker scan, temporary-artifact/process checks, and Secret Guard staged/tracked/gitignore scans.
- LSP diagnostics were attempted before and after source edits but remained unavailable because the isolated worktree paths are outside the request cwd. Markdown/text evidence files have no configured LSP server.
- Atomic remediation commit: `f84e0ec55bc30984be186156a8d10e475082eff4` (`test(core): isolate echo policy exclusion`). Final product worktree is clean; the branch has no upstream and was not pushed.

## 2026-08-30T10:15:04+10:00 - Independent second-remediation review

- Verdict: APPROVE. The sole remaining Echo policy-exclusion blocker from the review of `dd69c17a72fee188b8c61f011b8fdb516a35a6a8` is closed at exact reviewed commit `f84e0ec55bc30984be186156a8d10e475082eff4`.
- Delta scope is exactly two test files: `crates/ma2a-core/tests/authorization_matrix.rs` registers the new module at lines 3-4, and new `crates/ma2a-core/tests/authorization_matrix/echo.rs` contains the focused regression. No production source, manifest, lockfile, or documentation changed in this second remediation.
- The isolated fixture at `echo.rs:8-12` grants caller and target `CapabilityGrants::ECHO` while setting `PolicyGrants::NONE`. The sole Space is constructed with `Revocation::None` at lines 18-21; the exact request is `RemoteOperation::ECHO_CALL` and the slice contains exactly `[shared_space]` at lines 24-27; the assertion compares `decision.err()` exactly with `Some(AuthorizationDenied::ACCESS_DENIED)` at lines 29-30. No revocation, absent membership, second predicate, or independent allowing Space can explain the denial.
- Structural uniqueness passed: ast-grep found exactly one `mod echo;` registration and exactly one `echo_is_denied_when_sole_shared_space_policy_excludes_echo` test; repository text search found no duplicate or shadow registration/test.
- Independent mutation proof passed: temporarily changed only `echo.rs:11` from `PolicyGrants::NONE` to `PolicyGrants::ECHO`. The 17-test matrix failed only `echo::echo_is_denied_when_sole_shared_space_policy_excludes_echo` at `echo.rs:30`, with `left: None` and `right: Some(AuthorizationDenied)`, proving authorization became allowed for the otherwise-complete Space. The mutation was completely reverted before final verification.
- Restored-state verification passed: `cargo fmt --all -- --check`; authorization matrix 17/17; real-Iroh zero-Space isolation 2/2; `cargo run --locked -p xtask -- check-loc`; locked aggregate `xtask check` with 222/222 Rust and 44/44 Web tests; `GIT_MASTER=1 git diff --check 39064083b10ae89b90315041c323dbe9d5e1ac6f..HEAD`; marker scans; Secret Guard staged and 275-file tracked scans.
- LSP diagnostics were attempted for both changed Rust files and rejected both isolated `/tmp/opencode` paths as outside the request cwd. Compiler, rustfmt, focused tests, LOC, and aggregate gates are the fallback evidence.
- Final hygiene passed: no product process or `/tmp/ma2a-zero-space-isolation-*` artifact remained. The Todo 10 product worktree is clean on branch `todo-10-authz` at exact HEAD `f84e0ec55bc30984be186156a8d10e475082eff4`; the deliberate mutation is absent.
