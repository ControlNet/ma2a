# Issues — ma2a-phase-0-1

Problems and gotchas encountered during work on this plan.

_Auto-scaffolded by /start-work. Append new entries below - never overwrite._

---

## 2026-08-31 - Todo 18 semantic correction limitations

- `lsp_diagnostics` rejects files in `/tmp/opencode/ma2a-todo-18-snapshot-sse` because they are outside the request cwd. Cargo check, strict Clippy, rustfmt, nextest, xtask, Biome, and TypeScript checks provide diagnostics in the actual worktree.
- Current-revision live SSE revocation is inherently racy through separate shell requests because login and authenticated projection activity can advance revision. The real-router integration test deterministically proves typed revocation closure, eight-stream admission, ninth-stream 429, and dropped-body permit release.

## 2026-08-31 - Todo 18 terminal recovery verification limitation

- `lsp_diagnostics` again returned `LSP file path must be inside request cwd` for both changed files in `/tmp/opencode/ma2a-todo-18-snapshot-sse`. Rustfmt, focused compiler tests, strict Clippy, the Rust no-excuse rule scanner, and the locked aggregate gate passed in the actual worktree.

## 2026-08-31 - Todo 18 Oracle correction verification notes

- The first supervised daemon attempt exited because `/tmp/opencode/ma2a-todo18-correction-qa` was not mode `0700`; correcting only the isolated QA directory permissions allowed normal startup and is not a product-source defect.
- `lsp_diagnostics` still rejected the isolated `/tmp/opencode/ma2a-todo-18-snapshot-sse` source directory as outside request cwd. Full Runtime compilation/tests, strict Clippy, rustfmt, diff checks, and the locked aggregate gate passed in that worktree.
- The dependency audit continues to print existing duplicate transitive-version warnings, and Web tests print existing jsdom canvas notices; advisories, bans, licenses, sources, unused dependencies, and all tests passed.

## 2026-08-29 - Todo 1 workspace bootstrap

- Rust LSP diagnostics timed out at the MCP 30-second limit even after compiler warm-up. `cargo check`, strict Clippy, rustfmt, and nextest all passed.
- Context7 documentation lookup was blocked by the configured monthly quota; dependency versions and behavior were verified from the locked build graph and local tool output.
- The Todo 1 binary intentionally implements only `--version`; `--help` and unknown arguments currently exit successfully without output. Full CLI behavior belongs to later plan todos.

## 2026-08-29T03:20:20+10:00 - Independent Todo 1 AdversarialVerify

- `.github/workflows/ci.yml:5` filters push CI to `main`, but the repository and `origin/HEAD` use `master`; default-branch pushes bypass CI.
- `docs/platform-support.json` contains future intentions rather than checked command/error output for deferred ARM64 compile, package, and smoke combinations. The CI architecture job is `continue-on-error` and persists no evidence.
- `xtask/src/pins.rs:127-130` accepts a lexically root-prefixed path without canonicalizing it or proving it is a workspace member. A temporary `../../../../tmp/opencode/ma2a-external-probe` dependency passed `cargo xtask check-pins`.
- `ma2a-app/build.rs` returned only `Error: MissingBun` when Bun was absent; returning `Result<(), BuildError>` causes the runtime to print `Debug`, so the actionable `Display` message is not shown.
- Advisory records disagree: `deny.toml` and `docs/dependencies.md` map RUSTSEC-2024-0436 to `paste`, while `learnings.md` says `number_prefix`.
- `docs/dependencies.md` does not enumerate every exact frontend direct dependency recorded in `web/package.json`, including `react-doctor`, `react-grab`, and `react-scan`.
- Secret scanning found no credentials in tracked files or the 49 non-ignored Todo 1 product files, but `.gitignore` covers none of the 20 common sensitive-file patterns audited by secret-guard.
- `Cargo.lock` and `web/bun.lock` exist but are not Git-tracked; `git ls-files Cargo.lock web/bun.lock` returned no output.

## 2026-08-29T03:45:16+10:00 - Todo 1 verifier remediation

- Fixed the default-branch CI gap by accepting pushes to both `master` and `main`.
- Replaced prospective ARM64 support text with schema 2 observed probe evidence and made CI architecture probes persist deterministic artifacts without job-level `continue-on-error`.
- Fixed the path-policy bypass by canonicalizing dependency paths and requiring membership in `cargo metadata.workspace_members`; regression tests cover valid, external, and malformed-version paths.
- Fixed missing-Bun diagnostics at the real build-script process boundary; the integration test observes the required Bun version and frozen-install recovery command on stderr.
- Completed the frontend direct-dependency inventory, expanded sensitive-file ignore coverage, and preserved safe example-file exceptions.
- All aggregate, test, web-build, distribution, explicit release-build, and isolated-binary smoke gates passed after remediation.
- Rust/YAML/JSON LSP requests remain unavailable because the daemon times out at 30 seconds; compiler, Clippy, rustfmt, nextest, Biome, TypeScript, JSON policy parsing, and live surface checks provide independent evidence.
- The committed-lockfile criterion cannot become true before the initial repository commit because the entire bootstrap is untracked. Both lockfiles exist, are not ignored, and remained byte-identical through verification.

## 2026-08-29T04:42:11+10:00 - Todo 2 core contracts

- `lsp_diagnostics` rejects the isolated `/tmp/opencode/ma2a-todo-2` worktree as outside the tool
  session cwd before rust-analyzer runs. Diagnostics were attempted for every changed Rust file;
  compiler, strict Clippy, rustfmt, nextest, pin, and LOC gates passed.

## 2026-08-29T05:00:14+10:00 - Independent Todo 2 AdversarialVerify

- `Decoder::read_small_uint` accepts only direct CBOR integers `0..=23`, so canonical u8 protocol
  versions `24..=255` (`0x18,value`) return `InvalidInput` instead of the documented
  `VersionMismatch` on both request and response decoders. Disposable full-domain probes failed at
  24 and explicit probes reproduced the same result at 255; committed tests cover only request
  majors `2..=23` and have no symmetric response version property.

## 2026-08-29T05:07:01+10:00 - Todo 2 canonical-version resolution

- Resolved the verifier finding with a version-only canonical u8 decoder: direct values `0..=23`
  and minimal `0x18,value` values `24..=255` now reach exact `1.0` comparison on request and response.
- Unsupported canonical u8 pairs return `VersionMismatch`; malformed, non-minimal, larger-width,
  negative, float, tag, text, array, and truncated encodings remain `InvalidInput`.
- Symmetric request/response tests cover explicit 24/255 boundaries and every major/minor pair in
  the full u8 domain. All focused, workspace, pin, LOC, dependency, and secret gates passed.

## 2026-08-29 - Todo 3 store schema v1

- Context7 lookup for `rusqlite 0.40.2` was blocked by the monthly quota; the exact locked crate
  source and compiled API behavior were used instead.
- `lsp_diagnostics` rejects files in `/tmp/opencode/ma2a-todo-3` as outside the tool session cwd
  before rust-analyzer starts. Diagnostics were attempted for every changed Rust file; rustfmt,
  strict Clippy, nextest, locked workspace checks, release build, and a live library probe passed.

## 2026-08-29 - Todo 4 local API v1

- Context7 lookup for `serde_json` was blocked by the monthly quota; the exact locked dependency and compiled API behavior were used instead.
- `lsp_diagnostics` rejected the isolated `/tmp/opencode/ma2a-todo-4` worktree as outside the tool session cwd before starting rust-analyzer or TypeScript diagnostics. Rust compiler, strict Clippy, rustfmt, nextest, Biome, TypeScript, LOC, dependency, build, and live-driver gates passed.

## 2026-08-29 - Todo 3 immutable-publication and crash-recovery resolution

- Reproduced the protected-key check-then-rename race with two barrier-started writers: both returned success and the later Unix rename replaced the first publication.
- Replaced rename publication with a same-directory hard link, whose destination creation is atomic and no-replace across processes; `AlreadyExists` maps to `ProtectedKeyAlreadyExists`, and temporary files are removed on publication success and failure paths.
- Replaced normal connection-drop recovery coverage with a child-process test that signals after real uncommitted mutations, is forcibly killed without sleeps, and proves repository reopen rolls back both row and revision.
- Rust LSP remained unavailable for the isolated worktree due the known request-cwd rejection; formatting, strict Clippy, all 13 store tests, and the full locked `xtask check` passed.
- 2026-08-29 Todo 3 verifier remediation evidence: the new private regression first failed at `assert!(first_result.is_ok())` after a real successful hard-link plus injected `PermissionDenied` from temporary unlink. After making only post-publication unlink best-effort, `cargo nextest run -p ma2a-store --lib` passed 1 test, focused integration tests passed 10 tests, full `ma2a-store` nextest passed 14 tests, and `cargo run --locked -p xtask -- check` completed all aggregate gates successfully.
- 2026-08-29 Todo 3 cross-target limitation: `cargo check --locked -p ma2a-store --target aarch64-pc-windows-msvc --lib --tests` could not reach `ma2a-store` validation on this Linux host because a transitive native build could not locate MSVC tooling including `lib.exe`. This is an environment/toolchain limitation, not an observed source failure.

## 2026-08-29 - Todo 6 Runtime Endpoint

- `lsp_diagnostics` rejects the isolated `/tmp/opencode/ma2a-todo-6` worktree as outside the request
  cwd before rust-analyzer starts. Rust compilation, strict Clippy, rustfmt, 69 workspace tests, and
  real Iroh integration tests passed.
- The locked aggregate dependency audit continues to report the existing duplicate transitive
  versions from the pinned Iroh graph as warnings; advisories, bans, licenses, and sources pass.

## 2026-08-29 - Todo 6 authoritative-state verifier remediation

- Reproduced actor/storage divergence when `observe_memberships` mutated in-memory memberships before
  persistence: a rejected SQLite update left storage unchanged while `status()` exposed the candidate.
- Reproduced stale unclean-shutdown evidence: the actor returned revision 3 after its final not-ready
  observation committed revision 4.
- LSP diagnostics again rejected all changed files because the Todo 6 `/tmp` worktree is outside the
  request cwd. Rust compiler, strict Clippy, focused regressions, and aggregate gates passed.

## 2026-08-29 - Todo 7 independent verification limitations

- `lsp_diagnostics` rejected every changed Rust file in `/tmp/opencode/ma2a-todo-7` because the
  isolated worktree is outside the request cwd. Rustfmt, strict Clippy, focused tests, and the full
  aggregate gate passed instead.
- Native Windows DACL and named-pipe impersonation execution is unavailable on this Linux host.
  Cross-checking `aarch64-pc-windows-msvc` stops in transitive C dependencies before Runtime
  compilation because the host lacks Windows ARM headers and a compatible C toolchain.
- Miri could not run because `cargo-miri` is not installed for the available nightly toolchain.
- The aggregate dependency audit continues to emit existing duplicate-version warnings from the
  pinned graph; advisories, bans, licenses, and sources all pass.

## 2026-08-29T15:38:23+10:00 - Todo 7 acceptance blocker

- Blocking: `crates/ma2a-runtime/src/ipc/server_tests.rs::conflicting_shutdown_dispatch_does_not_request_server_shutdown` does not exercise a live `LocalApiServer`. It seeds replay state independently and checks the direct `dispatch` return, but never sends the conflicting framed shutdown request through the server and never proves a subsequent handshake/status request succeeds. Todo 7 remains `needs-fix` until the required server-survival regression is committed and passes.
- Platform limitation: native Windows execution remains unavailable. The installed AArch64 MSVC target stops in `ring`, `blake3`, and `libsqlite3-sys` because this Linux host lacks `clang`, Windows ARM headers, and `lib.exe`; no MA2A Windows source failure was observed before those blockers.
- Tool limitation: all changed-Rust LSP requests were rejected because the Todo 7 `/tmp` worktree is outside request cwd, and `cargo-miri` is not installed. Compiler, strict Clippy, rustfmt, nextest, dependency-source inspection, and runtime probes provide fallback evidence.

## 2026-08-29T16:10:35+10:00 - Todo 7 acceptance blocker resolved

- Resolved the sole Todo 7 blocker by replacing the direct `dispatch` regression with a live framed
  IPC test. It independently seeds replay conflict state, observes encoded `conflict`, proves the
  same live server accepts a subsequent status request, and exits only through explicit cancellation
  as `ServerExit::Cancelled`.
- A disposable restoration of the old operation-based shutdown predicate made the new regression
  fail with `ConnectionRefused` on the subsequent status call, proving sensitivity to the named bug.
- All required focused, package, workspace, formatting, strict Clippy, and locked aggregate gates
  pass. The known outside-request-cwd LSP limitation remains a tooling limitation, not an acceptance
  blocker.

## 2026-08-29 - Todo 8 tooling limitation

- `lsp_diagnostics` rejected every changed Rust path in `/tmp/opencode/ma2a-todo-8` because it is
  outside the request cwd. Rustfmt, strict Clippy, focused/workspace nextest, and locked aggregate
  checks are the fallback evidence.

## 2026-08-29 - Todo 9 final policy correction

- The Rust no-excuse checker treated `crates/ma2a-net/src/enrollment/tests.rs` as non-test code
  because it resides below `src/` and rejected its `Box<dyn Error>` alias. The harness now uses
  `std::io::Error` with explicit mappings at external async boundaries; enrollment behavior and
  assertions are unchanged.
- An earlier aggregate run observed a daemon lifecycle startup timeout, while the failing scope and
  the subsequent quiet full-workspace run passed. Following flaky-triage guidance, this is recorded
  only as a transient pre-existing lifecycle contention/race candidate; no Todo 9 causality is
  proven.
- `lsp_diagnostics` again rejected the isolated `/tmp/opencode/ma2a-todo-9` path as outside the
  request cwd. Rustfmt, compilation, strict Clippy, live Iroh tests, no-excuse policy, LOC, and full
  workspace nextest provide fallback evidence.

## 2026-08-29 - Todo 12 independent verification limitations

- `lsp_diagnostics` rejected both changed Rust files because the isolated Todo 12 worktree is outside
  the request cwd. Rustfmt, focused and workspace nextest, strict Clippy, all-target build, live Iroh
  dialing, and aggregate checks provide fallback evidence.
- `cargo deny check` passed advisories, bans, licenses, and sources while continuing to report the
  existing duplicate transitive versions inherited from the pinned Iroh dependency graph.

## 2026-08-30 - Todo 14 tooling limitation

- `lsp_diagnostics` rejected every changed Rust file in the isolated `/tmp` worktree as outside the request cwd. Rustfmt, compiler checks, strict workspace Clippy, targeted tests, nextest, and direct executable QA passed.

## 2026-08-30 - Todo 14 correction tooling

- The same request-cwd rejection affected all newly changed Runtime scheduling files. Focused compiler, strict Clippy, deterministic unit tests, real-Iroh E2E, LOC policy, and aggregate repository checks are used as independent fallback evidence.

## 2026-08-30 - Todo 14 final verification notes

- The first final aggregate run found `actor.rs` at 252 pure LOC after rustfmt expanded lint metadata. Consolidating imports reduced it below policy without behavior changes; the unchanged 268-test suite and aggregate gate then passed.
- Existing duplicate transitive dependency warnings from the pinned Iroh graph and jsdom canvas notices remained non-fatal; advisories, bans, licenses, sources, and accessibility tests passed.

## 2026-08-30 - Todo 14 second verifier tooling

- `lsp_diagnostics` again rejected every changed Rust path under `/tmp/opencode/ma2a-todo-14-control-sync` as outside the request cwd. Focused compilation, rustfmt, strict Clippy, no-excuse checks, LOC, 39 Store tests, 9 real-Iroh control-sync tests, and the locked aggregate gate provide fallback diagnostics.
- The first forwarding-test layout reached 272 pure LOC and strict Clippy reported indexing/slicing and parameter-count violations; splitting deterministic responder/snapshot support reduced both files below policy and all strict gates passed.

## 2026-08-31 - Todo 16 verification notes

- `lsp_diagnostics` rejected changed Rust files under the isolated `/tmp` worktree as outside the
  request cwd. Rust compilation, strict workspace Clippy, rustfmt, focused real-Iroh tests, the full
  serial workspace suite, and the locked aggregate gate passed as fallback diagnostics.
- A parallel full-suite run observed one credential-daemon startup timeout while an all-target build
  ran concurrently. The exact test passed immediately in isolation and the complete serial workspace
  run passed without source changes, identifying resource contention rather than a Todo 16 regression.
- Existing duplicate transitive dependency warnings from the pinned Iroh graph remain non-fatal;
  advisories, bans, licenses, sources, and unused-dependency checks passed.

## 2026-08-31 - Todo 17 remediation verification notes

- `lsp_diagnostics` rejected all ten changed Rust paths under `/tmp/opencode/ma2a-todo-17-echo` because they are outside the request cwd. Rustfmt, compiler checks, strict Clippy, no-excuse checks, focused tests, full nextest, LOC, and the locked aggregate gate passed instead.
- The locked dependency audit continues to report existing duplicate transitive versions from the pinned Iroh graph as warnings; advisories, bans, licenses, sources, and unused-dependency checks passed.
- `actor.rs` is compliant but remains in the warning band at 247 policy LOC; the next actor responsibility added there should be extracted rather than expanding the dispatcher file.

## 2026-08-31 - Todo 17 independent-verification correction

- The prior paused-time test asserted only `Elapsed` and missed that `handle_stream` discarded it, causing the peer to observe `Unavailable`. The replacement test reads the real response frame and distinguishes status `4` from stream closure.
- The prior cancellation evidence overclaimed peer-visible behavior from an internal oneshot receiver. It was removed and the protocol documentation now states the actual shutdown guarantee.

## 2026-08-31 - Todo 18 verification notes

- `lsp_diagnostics` cannot address changed Rust or TypeScript files in the isolated `/tmp` worktree because the tool request root is `/home/zhixi/GitRepos/ma2a`. Rust compiler, rustfmt, strict Clippy, focused and aggregate tests, Biome, and strict TypeScript checks provide fallback diagnostics.
- The production frontend currently has no confirmed rendered caller for the new Runtime snapshot/SSE client. Browser QA therefore covered the real embedded setup route and console, while API recovery behavior was exercised through focused TypeScript tests and the live loopback HTTP surface.
- The locked dependency audit and jsdom canvas notices remain existing non-fatal output; no Todo 18 source failure was observed from either warning class.

## 2026-08-31 - Todo 19 remediation verification notes

- `lsp_diagnostics` rejected changed Rust files in `/tmp/opencode/ma2a-todo-19-cli-workflows` as outside the request cwd. Rustfmt, compiler checks, strict workspace Clippy, focused tests, the locked aggregate gate, and direct process QA passed in the actual worktree.
- Native Windows execution is unavailable on this Linux host. The installed AArch64 MSVC target stops in transitive `blake3`, `ring`, and `libsqlite3-sys` C builds because compatible Windows headers, `clang`, and `lib.exe` are absent; the audited Windows source boundary is registered in the repository unsafe inventory.
- Miri could not run because `cargo-miri` is not installed for the available nightly toolchain. The new unsafe code is Windows-only FFI and was instead checked by reasoned per-call safety invariants, the repository audited-boundary inventory, native strict Clippy, and the attempted Windows cross-target build up to missing host C/MSVC tooling.
- One parallel full `cargo test` run observed `enrollment_denials` report a cross-Space ticket success. The exact test passed twice immediately in isolation and passed in the subsequent complete `xtask check` nextest run, so it is recorded as a transient integration race candidate rather than a Todo 19 regression.
- Existing duplicate transitive dependency warnings and jsdom canvas notices remain non-fatal; advisories, bans, licenses, sources, unused dependencies, all 352 Rust nextest cases, and all 54 Web tests passed.

## 2026-08-31 - Todo 21 correction verification notes

- `lsp_diagnostics` rejected the changed Rust files in `/tmp/opencode/ma2a-todo-21-phase-one-e2e` because they are outside the request cwd. Rustfmt, strict target Clippy, exact nextest, LOC, locked aggregate checks, and evidence-schema validation passed instead.
- The first post-commit three-run attempt completed two repetitions, then pre-existing Scenario A emitted its expected evidence but failed during teardown with Iroh `internal consistency error`. A clean full three-repetition rerun passed 46/46 each time without source changes, so this remains a transient relay teardown flake candidate rather than a correction regression.

## 2026-08-31 - Todo 21 integration blocker

- Blocking: the accepted Todo 21 candidate `578260d3306f549891074854cefba3312439b1cc` is based on
  Todo 18 `ead410f8f37a6c20e06b45180e956ca9d652d6db`, while current `master` includes accepted Todo 19
  through `f40b2608fd3e03c7f00efa281b2ee66c6ed33692`. `git merge-base --is-ancestor master candidate`
  fails, so the required fast-forward cannot be performed without dropping Todo 19 history or
  rewriting/cherry-picking the candidate, both prohibited by this task.
- Post-merge E2E, aggregate, formatting, Clippy, LOC, integrated-range diff, and main-worktree LSP
  verification were not run because no integration occurred. Candidate pre-merge audits confirmed a
  clean source worktree, zero merge commits, no forbidden committed paths, and no release-path changes.

## 2026-09-01 - Todo 21 integrated verification notes

- The previous ancestry blocker was resolved by rebasing the exact 20-commit Todo 21 range onto Todo 19 and fast-forwarding `master`; no Todo 19 history was dropped.
- The first integrated E2E run failed only because `recovery.rs` invoked human-readable `status` output and parsed it as JSON while expecting the obsolete result type `status`. Manual CLI evidence proved stale-socket recovery itself succeeded; the corrected test requests `--json` and asserts the current `snapshot` result type.
- `lsp_diagnostics` timed out twice for the changed root Rust test at the daemon's 30-second limit. The targeted regression, three 46/46 E2E runs, two aggregate checks, rustfmt, strict Clippy, and LOC policy all passed.
- Aggregate dependency audit output retains existing duplicate-version warnings from the pinned dependency graph, and frontend tests retain existing jsdom canvas notices; advisories, bans, licenses, sources, unused dependencies, and every test passed.

## 2026-09-01 - Todo 20 integrated verification notes

- The locked aggregate gate retains the existing pinned-dependency duplicate-version warnings, and JSDOM retains its existing canvas notices. Dependency advisories, bans, licenses, sources, unused-dependency analysis, frontend checks, and all tests passed.
- Two delegated final visual-review agents were cancelled after exceeding the harness's consecutive-read threshold. Fresh embedded-binary screenshots were instead inspected directly after browser geometry checks at 375, 768, and 1280 CSS pixels; the earlier mobile active-route visibility and relay-form label alignment defects were fixed and reverified.
- Final browser QA exercised external-termination Private Relay configuration. Native-TLS behavior remains covered by the typed form/action tests and Rust mutation/API contract tests rather than a second live certificate-backed browser run in the final root pass.
- `Cargo.lock` has an unrelated unstaged dependency-list ordering-only diff. It was not modified or staged during Todo 20 integration closeout.

## 2026-09-01 - Todo 20 embedded UI correction verification notes

- `lsp_diagnostics` timed out for both changed Rust modules at the daemon's 30-second limit. Rustfmt, strict target Clippy, the exact process-boundary Nextest target, Rust no-excuse rules, and the locked aggregate gate passed instead.
- The isolated browser-QA daemon was shut down through the product command and its Todo 20 state directory and empty log were removed. Unrelated daemons from other worktrees were left untouched.

## 2026-09-01 - Todo 20 bounded recovery correction verification notes

- The first browser login locator assumed a password label, while the live accessible name is `Passphrase`; using the actual accessibility tree resolved the automation timeout without a product change.
- A post-revoke reporting script used an unavailable sandbox `URL` constructor after the destructive interaction completed. Direct page, network, cookie, and storage inspection confirmed the expected result: one successful revoke-all request, no logout request, `/login`, and no remaining cookies or browser storage.
- Existing dependency duplicate-version warnings and JSDOM canvas notices remain non-fatal. All focused, aggregate, compiler, Clippy, formatting, frontend, and production-build gates passed.

## 2026-09-01 - Todo 22 evidence limitations

- This host provided native Linux x86_64 release evidence only. Supported `x86_64-apple-darwin` and `x86_64-pc-windows-msvc` execution remains owned by the blocking native macOS and Windows workflow jobs; it was not locally executed or overclaimed.
- `aarch64-unknown-linux-gnu`, `aarch64-apple-darwin`, and `aarch64-pc-windows-msvc` remain deferred in the authoritative support matrix because this host lacks the required native/cross SDKs, C toolchains, and emulation/runtime proof.
- GitHub OIDC provenance and SBOM attestation creation cannot be executed locally. Evidence verifies workflow YAML, supported-target matrix derivation, native-runner mappings, and blocking build -> archive validation -> clean-room smoke -> attestation -> upload order only.
- Several unrelated pre-existing MA2A daemons from other worktrees were observed during cleanup and intentionally left untouched. The one Task 22 release-inspection daemon and its private state directory were shut down and removed.

## 2026-09-01 - Todo 22 correction verification notes

- Native macOS, Windows, ARM64, and GitHub OIDC attestation execution remain unavailable locally and were not overclaimed; the blocking CI jobs and deferred matrix evidence retain ownership of those surfaces.
- Rust LSP diagnostics repeatedly timed out at the daemon limit. Rustfmt, strict Clippy, all 19 xtask tests, release/support/pin/LOC checks, and real extracted-binary smoke passed instead.
- `cargo-dist` repeatedly reordered two `ma2a-app` dependency names in `Cargo.lock` without changing resolution. The ordering-only churn was restored after each native build and excluded from commit `247c82a`.

## 2026-09-01 - F3 real manual QA

- The final native archive's documented Echo invocation is unusable: `echo --endpoint ENDPOINT --text TEXT --json` and `echo --endpoint ENDPOINT --stdin` are rejected by the released clap contract because `--endpoint` is mutually exclusive with payload and JSON flags. This blocks the required Echo-by-Endpoint-ID user flow.
- Two independently launched local Runtimes redeemed an invitation and reported the peer in control-sync status, but targeted `space sync now` returned `unavailable` because no exchanged transport address was available in this standalone CLI setup; no mock transport was substituted.

## 2026-09-01 - Todo 22 ws/wss fixture evidence notes

- The final fixture-only correction was validated on native Linux x86_64. Native macOS, Windows, ARM64, and GitHub OIDC attestation execution remain unavailable locally and were not overclaimed.
- Rebuilding and metadata generation again produced only the known `Cargo.lock` dependency-name ordering churn; it was restored and excluded from commit `c8676ea`.
- Shell LSP diagnostics, shell/Perl syntax, direct scanner fixtures, all 19 xtask tests, release/support/pin/LOC gates, archive validation, authenticated native smoke, metadata validation, and release failure probes passed.

## 2026-09-01 - F2 quality and security audit blockers

- HIGH: inbound `ma2a/control/1` handlers read and retain up to 1 MiB before shared-Space authorization and have no global/per-peer admission permit. The bounded Runtime mailbox does not cap handlers or bodies already waiting to send, so concurrent unauthorized connections can exhaust memory before ten-second timeouts release it.
- MEDIUM: `LocalApiServer` retains every successful client-selected mutation request ID and cloned result in an unbounded process-memory `BTreeMap`. There is no eviction or persistence, so memory grows for the daemon lifetime and restart violates the documented durable replay contract.
- `ControlRoundQueue.waiters` was investigated separately and is not an independent unbounded remote path: local connection limits and round deadlines bound reachability, and completed rounds remove assigned waiters.
- Representative Rust LSP diagnostics timed out. Fresh rustfmt, strict Clippy, 431/431 nextest, aggregate xtask, dependency, frontend, secret, marker, and unsafe-inventory gates passed.

## 2026-09-01 - F2 durable replay remediation notes

- The local mutation replay finding is remediated with schema-v4 persistence, count and byte budgets, deterministic completed-row eviction, exact response replay, conflict classification, and live restart tests.
- An interrupted reservation intentionally remains pending and returns `unavailable` for an identical retry. This is fail-closed for heterogeneous mutations, including external effects that cannot share one SQLite transaction.
- Changed-file Rust LSP diagnostics repeatedly timed out at the daemon's 30-second limit; compiler, focused Nextest, rustfmt, strict Clippy, LOC, and aggregate gates provide fallback diagnostics.

## 2026-09-01T14:02:49+10:00 - F2 inbound control admission verification notes

- Rust `lsp_diagnostics` repeatedly timed out at the daemon/MCP 30-second limit for the changed Net, Runtime, and App paths. Rust compiler checks, strict Clippy with warnings denied, rustfmt, focused real-Iroh Nextest, repository scripts, and diff checks provide fallback diagnostics.
- One earlier concurrent `./scripts/check.sh` invocation observed an enrollment timing failure while the same test passed in the complete test suite and the unchanged check passed when rerun alone. This is recorded as resource contention rather than a control-admission regression.
- Aggregate tooling can reorder two dependency names in `Cargo.lock` without changing resolution. The ordering-only churn was restored and the lockfile is excluded from the remediation.

## 2026-09-01 - F3 Echo CLI contract correction

- `EchoArgs` now requires `--endpoint` independently and makes only `--text` and `--stdin` a required exclusive payload group, so documented Endpoint-addressed Echo commands can include `--json` without clap conflicts.
- Process-level binary tests cover Endpoint + text + JSON, Endpoint + stdin with and without JSON, dual payload rejection, missing endpoint/payload usage errors, and rejection of the obsolete Space address option.

## 2026-09-01T15:30:00+10:00 - Todo 22 final rebuild limitation

- The repository-defined `scripts/smoke-release.sh` still calls removed `ma2a web` and exits 2 on the final archive. A disposable copy changing only that command to `ma2a ui open` passed the complete offline asset, login, authenticated snapshot, logout, and original-cookie revocation flow. This evidence-only task did not modify tracked source.
- The unchanged release failure probes pass, and a disposable canonical-CLI copy also passes, proving the checksum-valid external-reference mutation reaches the intended scanner gate with the final CLI.
- Native macOS/Windows x86_64, ARM64, and GitHub OIDC execution remain unavailable locally and were not overclaimed. Cargo tooling again produced only the known lockfile ordering churn, which was restored to HEAD.

## 2026-09-01 - Todo 22 tracked smoke correction limitations

- Native macOS, Windows, ARM64, and GitHub OIDC attestation execution remain unavailable locally; this correction was validated on the native Linux x86_64 archive and leaves those CI-owned surfaces unclaimed.
- The first attempted filtered Nextest invocation passed multiple test filters where Cargo accepts only one, so it exited before running tests; the valid focused xtask release-test commands were rerun separately afterward.

- Correction: the valid `cargo nextest run --locked -p xtask` command reran the complete 19-test xtask binary after that invalid filter attempt; individual Cargo test filters were not rerun separately.

## 2026-09-01 - Todo 22 final evidence correction limitations

- Native Linux x86_64 is the only release target executed locally. Native macOS and Windows jobs remain CI-owned; ARM64 targets remain deferred where recorded; GitHub OIDC provenance and SBOM attestation cannot be created or verified locally.
- Release tooling recreated the known `Cargo.lock` dependency-name ordering-only churn; it was restored to the exact final HEAD before final status verification.
- The evidence ZIP digest is recorded externally in its sidecar and completion receipt rather than inside the ZIP, avoiding a self-referential checksum.

## 2026-09-01 - Todo 22 checksum formatting correction limitations

- No additional release limitation was introduced. Native macOS/Windows, ARM64, and GitHub OIDC execution remain CI-owned or deferred as recorded in the evidence limitations.

## 2026-09-01 - Final-wave F4 CLI scope correction limitations

- Rust LSP diagnostics timed out at the 30-second daemon/MCP limit for all three changed Rust files; compiler, strict Clippy, rustfmt, focused process tests, LOC, and locked aggregate gates passed.
- Native macOS, Windows, ARM64, and GitHub OIDC verification were not attempted or overclaimed from this Linux x86_64 host.

## 2026-09-01T20:05:10+10:00 - Todo 22 exact-final-HEAD rebuild limitations

- Native Linux x86_64 is the only release target executed for the `c1165ecb48ea3deba3617a9b688667a2fa51229d` evidence rebuild. Native macOS and Windows remain CI-owned, ARM64 remains deferred where recorded, and GitHub OIDC provenance/SBOM attestations were not created or verified locally.
- Repository metadata generation emits absolute workspace paths in the dependency/license report. The evidence copy normalizes only those path strings to `/workspace/ma2a/`; package names, versions, license groupings, and SPDX package inventory remain unchanged.
- Cargo tooling recreated only the known `ma2a-app` dependency-name ordering churn in `Cargo.lock`; it was restored to the exact final HEAD and excluded from evidence. Unrelated pre-existing MA2A daemons were left untouched.

## 2026-09-01T21:08:06+10:00 - Final F3 refresh limitations

- Native Linux x86_64 is the only release target executed locally. Native macOS and Windows remain CI-owned, ARM64 remains deferred according to the support matrix, and GitHub OIDC provenance or attestation execution was not performed or inferred.
- Playwright MCP connections closed twice only when custom request/console event listeners were installed in `browser_run_code_unsafe`; standard navigation, screenshots, snapshots, console export, network export, and all product interactions remained stable. Fresh standard-tool contexts supplied the final zero-error authenticated and recovered evidence.
- Deliberate Runtime shutdown produced expected incomplete-stream and connection-refused browser errors. These are retained separately as offline-transition evidence and are not represented as clean-console failures.
- Unrelated pre-existing MA2A daemons from other worktrees were intentionally left untouched. Only the F3-created extracted runtime, browser contexts, owner-only state, and temporary files are cleanup-owned by this refresh.
