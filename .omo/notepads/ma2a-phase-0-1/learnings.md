# Learnings — ma2a-phase-0-1

Conventions, patterns, and successful approaches discovered during work on this plan.

_Auto-scaffolded by /start-work. Append new entries below - never overwrite._

---

## 2026-08-31 - Todo 18 semantic correction

- Restart-aware SSE continuity requires a private Runtime boot identity alongside revision; revision distance alone cannot distinguish a replacement daemon from a consecutive update.
- A bounded event queue needs capacity reserved for its terminal recovery event, otherwise overflow can prevent the required typed resynchronization signal.
- Control synchronization is queue/round health, not equality between unrelated endpoint and space cardinalities.
- Configured local direct addresses are capabilities, not evidence of an observed successful direct path; snapshot reachability must remain conservative until precise telemetry is owned by the projection.
- Live shell probes cannot reliably hold a current revision when authenticated session activity itself changes the projection; deterministic router tests are the correct surface for revocation closure, while live QA independently covers authentication and stale recovery.

## 2026-08-31 - Todo 18 terminal recovery proof

- Reserved-capacity tests must inspect serialized SSE frames, not channel length: eight ordered revisioned invalidations followed by an exact revision-19 `resync-required` frame proves the ninth slot cannot silently hold another ordinary event.
- Session expiry can be tested without sleeps by sharing the existing manual auth clock with the runtime router, advancing it directly to absolute expiry, and asserting the active stream emits only its typed recovery frame before EOF.
- Reusing the shared owner-only `TempState` fixture keeps integration coverage isolated while avoiding duplicated setup and preserving the repository source-size policy.

## 2026-08-31 - Todo 18 Oracle correction

- A snapshot carrying one durable Store revision must contain only values owned by that revision; actor-local observations need their own revision source or conservative projection values.
- Session invalidation is not a recoverable revision discontinuity: an SSE connection must close silently so it cannot disclose the last observed revision or any state after authorization is lost.
- Browser mutations can preserve one command contract by parsing the existing typed local API `Command`, checking route/operation agreement, and forwarding it through `LocalApiClient` after same-origin authentication and CSRF checks.
- Isolated Runtime QA directories must be exactly owner-only (`0700`); broader temporary-directory permissions are rejected before daemon storage opens.
- Independent re-review approved the correction once each prior High finding had direct source evidence, focused regressions, and live surface evidence rather than aggregate-test evidence alone.

## 2026-08-31 - Todo 19 Oracle remediation

- Secret-file validation must use one opened handle from identity checks through bounded reading; path metadata followed by a second open leaves a replacement window.
- Mutation adapters should propagate the Store's committed revision directly as a typed result so actor state, responses, and events remain one authoritative revision.
- Space names are durable signed membership data, not presentation defaults; snapshot reconstruction should read the local signed member label after restart.
- Private relay status is lifecycle state owned by the actor. Startup must reconstruct a persisted server before reporting online, and public relay status must remain based on observed connectivity rather than configuration alone.
- Process-level CLI regressions and direct terminal use both confirmed help precedence, secret non-reflection, owner-only invite creation, durable labels, and relay restoration.

## 2026-08-29 - Todo 1 workspace bootstrap

- Keep the frontend build inside `ma2a-app/build.rs` so release builds fail early and embed a frozen `web/dist` tree rather than depending on runtime source assets.
- Invoke `cargo-machete` directly from `xtask`; nested Cargo invocation can fail because Cargo exports workspace package environment variables to child commands.
- An inline empty favicon (`data:,`) keeps the compile-safe frontend shell free of runtime asset fetches and prevents a browser-console 404 during production-preview smoke tests.
- The pinned Iroh 1.1.0 graph currently includes unmaintained transitive crates `atomic-polyfill` and `number_prefix`; narrow `cargo-deny` exceptions and explicit documentation preserve an otherwise strict advisory gate.

## 2026-08-29T03:45:16+10:00 - Todo 1 verifier correction

- Correction to the earlier advisory note: `RUSTSEC-2024-0436` applies to the unmaintained `paste` crate, not `number_prefix`. `deny.toml` and `docs/dependencies.md` contain the authoritative mapping.
- Path-dependency policy must compare canonical paths against `cargo metadata.workspace_members`; lexical repository-prefix checks are vulnerable to traversal through `..`.
- Unsupported-target records should distinguish observed host-limited cross-compilation failures from native support conclusions and persist the command, exit code, outcome, output, and evidence host.

## 2026-08-29T04:42:11+10:00 - Todo 2 core contracts

- A narrow handwritten CBOR codec is smaller and stricter than a generic serializer for the closed
  Phase 1 envelopes: exact headers enforce definite lengths, ascending integer keys, minimal length
  forms, no floats/tags/indefinite values, and borrowed payload decoding before allocation.
- The workspace warns on exported exhaustive enums, while Todo 2 requires a closed wire set. Private
  discriminants behind public invariant-preserving wrappers keep the contract closed without
  `#[non_exhaustive]` or lint suppression.
- Stable binary vectors should be specified independently from implementation, checked in as raw
  `.cbor`, and verified through encode/decode/re-encode plus recorded SHA-256 hashes.

## 2026-08-29 - Todo 3 store schema v1

- Check the stored `user_version` before applying persistent connection settings such as WAL so a
  future schema is rejected without mutating its journal mode.
- Protected key persistence should combine opaque database references with owner-only immutable
  files, same-directory atomic rename, file and parent-directory sync, and zeroizing read buffers.
- Dynamic SQL for a pair of fixed schema tables is unnecessary; separate typed sequence-query
  helpers avoid parameter bloat and preserve static SQL reviewability.
- The full `cargo run --locked -p xtask -- check` gate exercises Rust, dependency policy, and web
  quality surfaces and should remain the final aggregate verification command.

## 2026-08-29 - Todo 4 local API v1

- Parse and reject incompatible local API versions before operation parsing or any Runtime callback.
- Fingerprint canonical typed commands for RequestId replay so semantically identical payloads reuse results and conflicting payloads fail before revision reads.
- Keep TypeScript generated payload models separate from exhaustive consumers when the formatter would otherwise exceed the 250-pure-LOC source ceiling.
- Best-effort SSE continuity is strict: disconnect, duplicate, reorder, or gap requires an authoritative snapshot.

## 2026-08-29T06:37:59+10:00 - Todo 5 adversarial frontend QA

- Real Chromium axe checks found serious CSS-dependent contrast failures that the passing jsdom axe assertions did not detect; browser accessibility evidence is required for rendered color validation.
- Reduced-motion overrides in an early imported stylesheet can be defeated by later animation and transition declarations, so the final cascade must be verified through computed styles.
- A skip link that works when focused programmatically may still be absent from normal sequential Tab traversal; test keyboard entry from a freshly loaded document.
- The production build, formatter, strict TypeScript check, and 29 tests passed, but the aggregate repository gate still failed because `web/src/styles/components.css` has 279 pure LOC against the 250-line policy.
- Horizontally scrollable mobile navigation needs a visible edge or scroll affordance; automatically centering the active item can clip both earlier and later route labels and hide discoverability even without document-level overflow.

## 2026-08-29 - Todo 5 verifier fix

- Mount-time `scrollIntoView` can alter Chromium's starting point for sequential focus even when it
  does not call `focus`; persistent current-route text is a safer mobile discoverability mechanism.
- A keyboard-scrollable region should be the named semantic landmark that actually owns overflow,
  not an unnamed wrapper around it; verify both axe output and real Page Down behavior.
- Reduced-motion rules should live in a final narrowly scoped stylesheet so equal-specificity
  animation and transition declarations cannot override them later in the cascade.
- Keep CSS under the pure-LOC policy by splitting coherent responsibilities such as content/state
  components and form/control primitives, rather than compressing unrelated selectors.
- Full-page screenshots can exceed the requested viewport height and weaken evidence consistency;
  viewport captures plus a separate interaction-state capture produce cleaner visual QA artifacts.

## 2026-08-29 - Todo 4 recursive contract proof

- A schema hash proves the schema copy is stable, not that handwritten serializers or language types implement it. Contract gates must walk actual serialized values or compiler types recursively.
- TypeScript compiler symbols expose optionality, literal unions, nested properties, array item types, and branded-string intersections without `any`, casts, or duplicate field inventories.
- Rust integration fixtures should cover complete discriminant inventories and validate real encoded JSON, so primitive drift such as numeric-to-string serialization fails at the same release gate as missing fields.

## 2026-08-29 - Todo 7 private IPC remediation

- On-demand Unix daemons can detach without `pre_exec` by launching a hidden exact-executable mode that calls safe `rustix::process::setsid()` before Runtime initialization; Windows uses `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP`.
- Startup lock, liveness, and shutdown polling should share a deterministic bounded exponential backoff rather than scheduler yields; a pure delay function keeps the schedule directly testable.
- Graceful shutdown signaling must derive from the accepted dispatch result, not the decoded operation, or replay conflicts can terminate the daemon after returning an error.
- Windows named-pipe privacy requires both a concrete current-user SID in the protected DACL and peer process-token SID equality; isolate the unavoidable token FFI behind a small safe platform crate so the Runtime retains `unsafe_code = "forbid"`.

## 2026-08-29 - Todo 4 semantic TypeScript identity proof

- Resolve a schema reference only after comparing the actual compiler alias symbol with an exported branded reference type; primitive `StringLike` equivalence cannot distinguish `EndpointId`, `SpaceId`, and `RequestId`.
- A numeric index type is not proof of an array because branded strings inherit string indexing. Require the compiler type to be `Array` or `ReadonlyArray` before inspecting its element type.
- A public response alias needs its own exact union check even when both concrete response envelopes are validated independently; compare the union's compiler alias members rather than duplicating fields.
- 2026-08-29 Todo 3 remediation: once `hard_link` succeeds, destination publication is already committed; failure to unlink the temporary alias must therefore be best-effort rather than reported as a failed write. Parent-directory sync and destination permission validation remain fatal and ordered after the cleanup attempt.
- 2026-08-29 Todo 3 testing: a private typed publication-operations seam can deterministically inject only post-link unlink failure while delegating the real hard-link, parent sync, and permission validation operations. This preserves production behavior and proves retry immutability without environment failpoints or timing assumptions.

## 2026-08-29T10:42:15+10:00 - Todos 3-5 integration

- Created `integration/todos-3-5-20260829T003047Z` at exact base `d4799c64cb9f9ca2966c54391906da490565d55e` in `/tmp/opencode/ma2a-integration-todos-3-5-20260829T003047Z`, then cherry-picked all 14 accepted commits individually in the requested Todo 3, Todo 4, Todo 5 order.
- Final integration and `master` HEAD is `007625727bbd960f16745b3cb97953c43641897d`. No cherry-pick conflicts occurred, no compatibility changes or additional commits were required, and the source branches were not rewritten.
- Source audit passed: no conflict markers, TODO/FIXME/HACK stubs, `as any`, TypeScript ignore directives, empty catches, production-library `unwrap`/`expect`, tracked `web/dist`, whitespace errors, or changed source files above 250 pure LOC.
- `cargo fmt --all -- --check` passed; `cargo nextest run -p ma2a-store` passed 14 tests; `cargo nextest run -p ma2a-runtime` passed 15 tests; `cargo nextest run --workspace --all-features` passed 63 tests.
- The first Web aggregate attempt selected cached Biome 2.3.14 because the fresh worktree had no `node_modules`; `bun install --frozen-lockfile` installed the locked local Biome 2.5.11 without changing manifests or lockfiles. The required `bunx biome check . && bunx tsc --noEmit && bun test && bun run build` then passed with 41 tests and a successful Vite production build.
- `cargo run --locked -p xtask -- check` passed at the isolated integration HEAD and passed again after `master` was fast-forwarded with `git merge --ff-only`.
- Real Chromium QA passed: sequential Tab reached the skip link, Enter focused `main#main-content`, route navigation worked without console errors, narrow navigation was named and horizontally scrollable, and reduced-motion computed styles had no active animations or transitions.
- Secret Guard staged scans found no staged files and tracked scans found no secrets, both before promotion and after the fast-forward.
- Main-path LSP diagnostics were attempted for changed store, runtime, and Web source/test directories; Rust requests timed out at the 30-second daemon limit and TypeScript requests timed out at the MCP limit. Compiler, nextest, Biome, TypeScript, browser, build, and aggregate gates provide the fallback proof.
- Final statuses: the integration worktree is clean; the main root remains on `master` at `0076257` with no tracked changes and only the pre-existing untracked `.omo/` evidence.

## 2026-08-29 - Todo 6 Runtime Endpoint

- Iroh 1.1 can enforce zero-Space protocol isolation before payload parsing by binding with only the
  enrollment ALPN; normal ALPN connection attempts fail negotiation.
- A dedicated bounded `spawn_blocking` command loop preserves single-owner rusqlite/key-store access
  without detached work or blocking Tokio workers.
- Splitting public state, error taxonomy, persistence ownership, and actor lifecycle keeps strict
  Rust modules under the 250-pure-LOC ceiling without compressing security contracts.
- Real restart and negotiation integration tests are fast enough for focused iteration: five
  identity/fail-closed cases complete in under 0.2 seconds and the Iroh scenario in about 0.14 seconds
  after compilation.
- A temporary SQLite `BEFORE UPDATE` trigger with `RAISE(ABORT, ...)` provides a deterministic,
  no-sleep persistence failure at the real Runtime API boundary while leaving the actor responsive.
- Candidate-state publication is the correct actor transaction pattern: clone current typed state,
  apply the command, persist, attach the committed revision, then swap and notify.
- Directly cancelling the private lifecycle token in a unit test deterministically exercises
  `finish(false)` and allows its acknowledgement revision to be compared with reopened repository
  metadata after both owned tasks join.

## 2026-08-29 - Todo 7 independent verification

- Named-pipe authorization should derive identity from the impersonated caller thread token, not a
  process identifier that can race with process exit or PID reuse.
- A module-scoped, reasoned `unsafe_code` allow under workspace `deny` keeps one audited FFI boundary
  visible and mechanically enforceable without creating a helper workspace package.
- Shutdown conflict tests are strongest when replay state is seeded independently; dispatching an
  accepted shutdown first couples the regression setup to the behavior under test.
- Multi-process singleton tests should compare a value served by the live daemon with independently
  reopened persisted state, rather than proving only that all clients received equivalent status.

## 2026-08-29 - Todo 6 integration

- Fast-forwarded `master` only from `007625727bbd960f16745b3cb97953c43641897d` to exact verified
  Todo 6 HEAD `d9e4130465a163acd800d0bd39abb5ba24d90262` with
  `GIT_MASTER=1 git merge --ff-only todo-6-runtime-endpoint`.
- The source was a strict linear descendant containing exactly ten ordered commits and no merge or
  committed `.omo/**` path; all ten commits remain intact on `master` without compatibility work.
- Integrated verification passed: `cargo nextest run -p ma2a-runtime` ran 22/22 tests,
  `cargo nextest run -p ma2a-app --test zero_space` ran 1/1, and
  `cargo run --locked -p xtask -- check` ran 71/71 Rust tests plus 41/41 Web tests and completed all
  formatting, Clippy, dependency, license, source, unused-dependency, install, type, and build gates.
- `git diff --check 007625727bbd960f16745b3cb97953c43641897d..HEAD` passed, and Secret Guard scanned
  147 tracked files with no secrets detected.
- Todo 8 and Todo 11 branches and worktrees remained at
  `007625727bbd960f16745b3cb97953c43641897d`; the main worktree has no tracked or staged changes and
  retains only the protected untracked `.omo/**` evidence, including this integration record.

## 2026-08-29 - Todo 8 signed Space manifests

- Complete revocation state must be validated across generations, not only as the delta of members
  removed by the current manifest; otherwise a revocation can silently disappear without re-addition.
- Denormalized SQLite chain metadata and authorization rows are part of the durable high-water
  boundary and must be compared with recomputed signed-chain state on every reopen.
- A protected-key-backed owner advancement API prevents downstream enrollment/control code from
  needing private authority bytes or opaque references outside the store boundary.

## 2026-08-29T16:23:21+10:00 - Todo 7 fast-forward integration

- Verified `master` at exact base `d9e4130465a163acd800d0bd39abb5ba24d90262`, then fast-forwarded
  with `GIT_MASTER=1 git merge --ff-only todo-7-private-ipc` to exact verified HEAD
  `63d520427b1db05fdd6c5e3b8c08eefbd4b4ccf9`; all 18 Todo 7 commits remain in order with no merge
  commit and no committed `.omo/**` path.
- Integrated verification passed the live shutdown-conflict regression (1/1), all Runtime tests
  (30/30), daemon lifecycle tests (6/6), tracked Secret Guard scan (162 files, no findings), and
  locked `cargo run --locked -p xtask -- check` including 41/41 Web tests. `git diff --check` passed.
- The required main-path LSP attempt for `crates/ma2a-runtime/src/ipc/server_tests.rs` timed out at
  the daemon limit. No `ma2a` daemon, IPC socket, or active lock remained after cleanup; one orphaned
  `/tmp/ma2a-session-1787976460` state directory was removed after confirming no process held it.
- Final main worktree has no tracked/staged changes and retains the pre-existing untracked `.omo/**`
  evidence; this entry was appended only and remains uncommitted.

## 2026-08-29T18:29:55+10:00 - Todo 8 rebase integration

- Verified the starting worktrees exactly: `master` was
  `63d520427b1db05fdd6c5e3b8c08eefbd4b4ccf9`, `todo-8-space-manifests` was
  `6f72a04fb7511ffb3be5cc874ba4ae0d9be870af`, and their pre-rebase merge base was
  `007625727bbd960f16745b3cb97953c43641897d`. The verified range contains 18 linear commits, not
  the requested count of 17; no verified commit was dropped or squashed.
- Rebased with `GIT_MASTER=1 git rebase --onto master 007625727bbd960f16745b3cb97953c43641897d`.
  Conflict 1 at old `507e196` retained Todo 6/7 `EndpointObservationUpdate` and `RuntimeMetadata`
  exports while adding Todo 8 `SpaceChainPersistence` and removing obsolete unsigned member DTO
  exports. Conflict 2 at old `14ea5fa` retained Runtime metadata loading and Endpoint observation
  persistence while removing only unsigned `upsert_member` and `revoke_member` mutation paths.
- Old-to-new Todo 8 mapping, in preserved order:
  `af000bff7a3acce82abd41db5a136fd43bf1d332` -> `4325e1d27bdb7e2db50346bcbf339d69d7020da7`;
  `032da409e74d6ae20a3adcf97471f70746e8d1ee` -> `33516d0d565d509690fb6287ddd1fdabd052b3f4`;
  `5614e2141c9766fc551cf94152a546a4ac0d2e04` -> `59744c953e206123c70e09ee15eb8ee315ff5748`;
  `af76c49dceaf83212ef052685e75dde0dbc1df4e` -> `0906407d4e170e33275ba4749997e6df87f98fa1`;
  `45dcc52c632b44d35bc567f7cf87202ca1ce40f2` -> `4992a20047decfde145db98d3b49370b34bbf47b`;
  `c766b6f38b67b503816a17517161ce95874e07a9` -> `6e15cfb3a15fba3a95caa7600b507eb8d01b9c10`;
  `6919a94f7472a0b1be0c4324c8492b17daaa84e8` -> `eb754b0db048b516cf793aa53c1a7a503ca4c0db`;
  `7ddb56fba23efd2c1a3544352cdc492b928531cd` -> `58bfbe1378ba71ec2b7dba57a1d29e8eb2fa4ef7`;
  `ba229adc11015e440ca9e7f6e464b2b6b71d6b88` -> `5390897e025b99adf5602fcc342d3efc9fe95814`;
  `507e196e4dc825cffb55ad1e05709d862204dbe8` -> `78828c53f8066a3fec61924f9b80a5aec00f8bf8`;
  `bc298b1e02271cc336f385a418083c0bb282b994` -> `462914bf48ee70a50929a6f037fcbaa929a7c068`;
  `cfc1c4208f183442d9823afd5a1fdf100c9d716a` -> `b3eae5aef89142f53c0539629e3af406c4d16668`;
  `14ea5faafa72ad235ecc12e08b59b608845b10a7` -> `3cf7420764c0f01dd78a5fd964d4985ec09f03d4`;
  `1d99744e9269e24793c60a6d83430e61bdfa89d5` -> `99c4f8c348b3817bacc12b1e8fa0a0edd49462d5`;
  `7e2e8e1ce8719f3475f65e69a72e422602ea7a4a` -> `da950e6e49bbbb75b7c534fb59dca71d062cb505`;
  `96d56d28232687e333cd4a04652e482f7a5d6af7` -> `f44536b1377b503eb81ee81c238e2f9d5a315c61`;
  `470687921861d54bb552dee3872d070c298e2437` -> `a19a53541c16c196da2ecf9eaccafec8294f5cd0`;
  `6f72a04fb7511ffb3be5cc874ba4ae0d9be870af` -> `5b508a361a79a81cfee661df66b34fc0ad1f7d07`.
- Focused rebased-branch verification passed `cargo nextest run -p ma2a-core` with 43/43 tests and
  `cargo nextest run -p ma2a-store` with 25/25 tests. `master` first fast-forwarded to the rebased
  Todo 8 tip `5b508a361a79a81cfee661df66b34fc0ad1f7d07`.
- The first locked aggregate attempt found only a rustfmt layout issue in the manually resolved
  `repository_state.rs` import. Because `master` was not permitted to be rewritten, the source branch
  received one reviewable integration-only commit,
  `874bd597c0892e51c05afaf426db9bdea1dba66e` (`style(store): format integrated state imports`), and
  `master` advanced again with `GIT_MASTER=1 git merge --ff-only todo-8-space-manifests`.
- Final verification passed `cargo run --locked -p xtask -- check`: 117/117 Rust tests and 41/41 Web
  tests passed, with formatting, strict Clippy, dependency, license, source, unused-dependency, type,
  and build gates successful. `GIT_MASTER=1 git diff --check 63d5204..HEAD` passed; the integrated
  range has 19 commits total, comprising the 18 mapped Todo 8 commits plus the integration-only
  formatting commit, no merge commits, and no committed `.omo/**` path.
- Secret Guard scanned 190 tracked files with no findings and reported no staged files. Required LSP
  diagnostics were attempted repeatedly at root for `crates/ma2a-store/src/lib.rs` and
  `crates/ma2a-store/src/repository_state.rs`, but the daemon timed out at its 30-second limit; Cargo,
  rustfmt, Clippy, nextest, and the aggregate gate provide fallback evidence.
- Final branch and `master` both point to `874bd597c0892e51c05afaf426db9bdea1dba66e`. No `ma2a` daemon,
  `/tmp/ma2a-session-*` directory, IPC socket, or lock remained. Both worktrees have no tracked or
  staged changes and retain only their pre-existing protected `?? .omo/` state; this record was
  appended only and remains uncommitted.

## 2026-08-29T22:41:03+10:00 - Todo 9 fast-forward integration

- Verified `master` was exactly `874bd597c0892e51c05afaf426db9bdea1dba66e` with no tracked or staged changes; only protected untracked `.omo/**` evidence was present.
- Verified `todo-9-enrollment` resolves exactly to `c7e3a992c75c19cc9dcec0aeaa117fbad460b9bc`, a strict 35-commit descendant with no merge commits and no committed `.omo/**` paths.
- Promoted with only `GIT_MASTER=1 git merge --ff-only todo-9-enrollment`; `master` now points exactly to `c7e3a992c75c19cc9dcec0aeaa117fbad460b9bc`, preserving all 35 commits and creating no merge commit.
- `cargo run --locked -p xtask -- check` passed: 135/135 Rust tests and 41/41 Web tests, with all aggregate formatting, Clippy, dependency, license, source, unused-dependency, type, and build gates successful.
- `GIT_MASTER=1 git diff --check 874bd597c0892e51c05afaf426db9bdea1dba66e..HEAD` passed. Secret Guard staged scan found no files; tracked scan checked 210 files and found no secrets.
- Final audit confirmed `master` at the exact target, no tracked or staged changes, no committed `.omo/**` paths, and protected untracked `.omo/**` evidence retained. No environmental limitation affected the required checks; dependency duplicate-version warnings remain non-fatal existing warnings.

## 2026-08-30T00:40:48+10:00 - Todo 11 rebase integration

- Verified the pre-rebase state exactly: `master` was `c7e3a992c75c19cc9dcec0aeaa117fbad460b9bc`, `todo-11-web-auth` was `d28481910661ad3bbd761f223ffe8f9151fd27dd`, and the old merge base was `874bd597c0892e51c05afaf426db9bdea1dba66e`. The source range contained exactly 34 ordered commits, zero merges, and no committed `.omo/**` paths.
- Rebased only the source worktree with `GIT_MASTER=1 git rebase --onto master 874bd597c0892e51c05afaf426db9bdea1dba66e`. The first conflict regenerated `Cargo.lock` from the Todo 9 side plus the exact Todo 11 dependency manifests, preserving both enrollment dependencies (`iroh-base`, `rusqlite`) and Web-auth dependencies (`argon2`, `rpassword`). The second conflict in `crates/ma2a-runtime/src/actor.rs` retained Todo 9 invite creation, redemption, cancellation, owner clock, and enrollment-call dispatch while adding Todo 11 authoritative `AdoptRevision` handling.
- The rebased branch ended at `bd8d6e6803749e95895ec94f71c71d39b71c98bd`. `git range-diff` mapped all 34 old commits to 34 new commits in order, including the Settings correction `28feca7 -> 244db27` and Web limits split `d284819 -> bd8d6e6`; no commit was dropped, squashed, or reordered, and no compatibility commit was required.
- Focused Todo 9 enrollment verification passed 24/24 tests: Core 7/7, Net 6/6, Store 2/2, Runtime 1/1, and App/Iroh 8/8, including replay denial and contiguous-generation establishment. Todo 11 focused verification passed 20/20 Rust tests, including Web security/limits 5/5, plus full Runtime 46/46 and Web 44/44 with Biome, strict TypeScript, and production build.
- Rustfmt, focused strict Clippy, and `xtask check-loc` passed; the manually resolved actor is 232 pure LOC. LSP diagnostics were attempted for `actor.rs` but rejected the isolated `/tmp` worktree as outside the request cwd, so compiler, Clippy, rustfmt, nextest, and aggregate checks provide fallback evidence.
- The first locked branch aggregate run had one transient `status_autostarts_one_private_daemon` startup timeout during the 159-test parallel suite. The exact test immediately passed alone in 0.205 seconds without changes, no daemon or state residue remained, and unchanged quiet reruns passed completely on both the rebased branch and integrated root: 159/159 Rust and 44/44 Web, with formatting, strict Clippy, dependency, license, source, unused-dependency, type, and build gates successful.
- Promotion used only `GIT_MASTER=1 git merge --ff-only todo-11-web-auth`; final `master` and the source branch both point to `bd8d6e6803749e95895ec94f71c71d39b71c98bd`. `git diff --check` passed, the integrated range has 34 commits and zero merges, and no `.omo/**`, `target`, or `web/dist` path is committed.
- Secret Guard staged scan found no files before promotion; tracked scan checked 244 files after promotion and found no secrets. Final root retains only the protected untracked `.omo/**` evidence, including this append-only integration record.

## 2026-08-30T05:45:50+10:00 - Todo 12 source rebase

- Verified the pre-rebase inputs: `master` was `bd8d6e6803749e95895ec94f71c71d39b71c98bd`, the accepted Todo 12 tip was `f97b25882ed46ffade5544ca2589a44133785a70`, and the old base was `874bd597c0892e51c05afaf426db9bdea1dba66e`. The source range contained exactly 24 ordered commits.
- Rebased only `todo-12-address-records` with `GIT_MASTER=1 git rebase --onto master 874bd597c0892e51c05afaf426db9bdea1dba66e`, preserving Todo 9 enrollment, Todo 11 Web auth, and Todo 12 signed address-record behavior through six semantic conflict resolutions.
- `git range-diff` mapped all 24 original commits to 24 rebased commits in order. Two explicit integration-only commits follow the mapped range: `0d17b7dd5c1a09ff7e9475c40782a48eac20be3f` (`chore(integration): reconcile address runtime bindings`) and `39064083b10ae89b90315041c323dbe9d5e1ac6f` (`refactor(net): type lookup-aware endpoint binding`).
- The final branch is a strict 26-commit linear descendant of `master`, with zero merge commits, no committed `.omo/**`, `target/**`, or `web/dist/**` paths, and no `git diff --check` findings. `master` remains unchanged at `bd8d6e6803749e95895ec94f71c71d39b71c98bd`; the source tip is `39064083b10ae89b90315041c323dbe9d5e1ac6f`.
- Focused verification passed Core 59/59, Store 30/30, Net 37/37, Runtime 46/46, App 23/23, and the Net doctest 1/1. Formatting, strict workspace Clippy, and the locked all-target/all-feature workspace build passed.
- `cargo run --locked -p xtask -- check` passed all aggregate gates: 203/203 Rust tests, dependency advisories/bans/licenses/sources, unused-dependency analysis, and 44/44 Web tests. Existing duplicate-version dependency warnings remained non-fatal.
- Secret Guard scanned 266 tracked files with no findings and reported no staged files. LSP diagnostics could not address the isolated `/tmp` worktree because it was outside the request cwd; rustfmt, Clippy, compiler, focused tests, and the aggregate gate provide fallback evidence.
- This entry is append-only protected evidence and remains uncommitted. The Todo 12 branch was not pushed or merged into `master`.

## 2026-08-30T06:04:00+10:00 - Todo 12 fast-forward integration

- Reconfirmed the exact promotion preconditions: root was on `master` at `bd8d6e6803749e95895ec94f71c71d39b71c98bd` with no tracked or staged changes and only protected untracked `.omo/**`; `todo-12-address-records` was exactly `39064083b10ae89b90315041c323dbe9d5e1ac6f`.
- The source was a strict 26-commit descendant of `master`, with zero merge commits and no committed `.omo/**`, `target/**`, or `web/dist/**` path. Promotion used only `GIT_MASTER=1 git merge --ff-only todo-12-address-records` and created no merge or compatibility commit.
- Final `master` and `todo-12-address-records` both resolve to `39064083b10ae89b90315041c323dbe9d5e1ac6f`. All 26 commits remain intact and ordered above the previous master `bd8d6e6803749e95895ec94f71c71d39b71c98bd`.
- Integrated-root `cargo run --locked -p xtask -- check` passed all gates: 203/203 Rust tests, 44/44 Web tests, formatting, strict Clippy, dependency advisories/bans/licenses/sources, unused-dependency analysis, TypeScript, and build checks.
- Existing duplicate transitive dependency warnings inherited from pinned Iroh remained non-fatal; advisories, bans, licenses, and sources all passed. Web tests also emitted the existing jsdom `HTMLCanvasElement.getContext()` not-implemented notices while all accessibility tests passed.
- Secret Guard scanned 266 tracked files with no findings and reported no staged files. `GIT_MASTER=1 git diff --check bd8d6e6803749e95895ec94f71c71d39b71c98bd..HEAD` passed.
- Final root has no tracked or staged changes and only protected `?? .omo/`; the source worktree is clean. This append-only integration record remains untracked and uncommitted.

## 2026-08-30T10:29:57+10:00 - Todo 10 fast-forward integration

- Reconfirmed the independently approved source evidence and exact promotion guards: root was on
  `master` at `39064083b10ae89b90315041c323dbe9d5e1ac6f` with no tracked or staged changes and only
  protected untracked `.omo/**`; the clean source worktree and `todo-10-authz` both resolved to
  `f84e0ec55bc30984be186156a8d10e475082eff4`.
- The approved source was a strict eight-commit linear descendant with merge base
  `39064083b10ae89b90315041c323dbe9d5e1ac6f`, zero merge commits, and no committed `.omo/**`,
  `target/**`, or `web/dist/**` path. Promotion used only
  `GIT_MASTER=1 git merge --ff-only todo-10-authz`; no rebase, cherry-pick, squash, amend, push,
  compatibility commit, or manual tracked-file edit occurred.
- Final `master`, `todo-10-authz`, and root `HEAD` all resolve exactly to
  `f84e0ec55bc30984be186156a8d10e475082eff4`. `GIT_MASTER=1 git diff --check
  39064083b10ae89b90315041c323dbe9d5e1ac6f..HEAD` passed and all eight approved commits remain
  intact in order.
- Integrated verification passed `cargo fmt --all -- --check`; authorization matrix 17/17;
  real-Iroh zero-Space isolation 2/2; strict workspace all-target/all-feature Clippy with
  `-D warnings`; `cargo run --locked -p xtask -- check-loc`; and locked aggregate `xtask check`
  with 222/222 Rust tests and 44/44 Web tests. Advisories, bans, licenses, sources, unused-dependency,
  formatting, TypeScript, and build gates passed; existing pinned-Iroh duplicate-version warnings
  remained non-fatal.
- Secret Guard staged, tracked, and gitignore modes passed: no staged files, 275 tracked files with
  no secrets, and all common sensitive patterns covered. Main-root LSP diagnostics were attempted
  for `crates/ma2a-core/tests/authorization_matrix.rs` and
  `crates/ma2a-core/tests/authorization_matrix/echo.rs`; the first timed out after 30000 ms at the
  daemon and the second returned `MCP error -32001: Request timed out`.
- Adversarial checks passed for stale state, dirty worktree, and misleading success output through
  exact independent refs, merge-base, ancestry, exit-code, test-count, diff, and final-status checks.
  Malformed-input, prompt-injection, cancel/resume, hung-command, flaky-test, and repeated-interruption
  were not applicable because no trigger fact appeared and every required command completed once.
- Cleanup passed: no process named `ma2a` and no `/tmp/ma2a-zero-space-isolation-*` artifact remained.
  Final root has no tracked or staged changes and retains only protected untracked `.omo/**`, including
  this append-only record and `.omo/start-work/ledger.jsonl`; the source worktree remains clean.

## 2026-08-30T10:57:10+10:00 - Todo 13 source rebase

- When a feature branch predates centralized protocol isolation, an export conflict should retain the
  newer closed-set isolation constant and add the feature exports; dropping either side silently weakens
  a reviewed security boundary.
- A range-diff `!` marker can be the expected integration proof rather than drift when it is confined to
  one explicitly reviewed conflict and records the exact newer-base symbol retained.
- Repeated real Iroh HTTPS client tests remain fast enough to run three times after a history rewrite,
  providing stronger evidence than listener startup or compilation alone.

## 2026-08-30T14:09:53+10:00 - Todo 13 fast-forward integration

- Reconfirmed the exact promotion guards before mutation: root `master` and `HEAD` were
  `f84e0ec55bc30984be186156a8d10e475082eff4`; `todo-13-private-relay` and its worktree `HEAD` were
  `6e68a39c1bf2101179d4653c212d9b0f1ba2b904`; the merge base was the expected master; and the source
  was a strict 15-commit linear descendant with zero merges and no committed `.omo/**`, `target/**`,
  or `web/dist/**` path.
- Promotion used only `GIT_MASTER=1 git merge --ff-only todo-13-private-relay`. Final `master`, root
  `HEAD`, and the source branch all resolve to `6e68a39c1bf2101179d4653c212d9b0f1ba2b904` with all 15
  reviewed commits intact and no merge or rewritten commit.
- Integrated focused verification passed authorization 17/17, zero-Space isolation 2/2, and relay
  behavior 21/21. The relay suite included one successful native-TLS real Iroh HTTPS client and one
  successful externally terminated real Iroh HTTPS client.
- Locked `cargo run --locked -p xtask -- check` passed all aggregate gates with 243/243 Rust tests and
  44/44 Web tests; dependency advisories, bans, licenses, sources, unused-dependency, formatting,
  Clippy, TypeScript, and build checks passed. Existing pinned-Iroh duplicate-version warnings and
  jsdom canvas notices remained non-fatal.
- `GIT_MASTER=1 git diff --check f84e0ec55bc30984be186156a8d10e475082eff4..HEAD` passed. Secret
  Guard staged, tracked, and gitignore modes passed: no staged files, 292 tracked files with no
  secrets, and complete common-sensitive-pattern coverage.
- Adversarial stale-state, dirty-worktree, and misleading-success-output probes passed through exact
  refs, ancestry, merge/count/path audits, independent tracked/staged checks in both worktrees, exit
  codes, and explicit test totals. No trigger appeared for malformed input, prompt injection,
  cancel/resume, hung commands, flaky tests, or repeated interruptions.
- Cleanup found zero matching MA2A/relay/test processes, TCP listeners, Unix listeners, or
  `ma2a-relay-tls-path-*`, `ma2a-net-*`, and `ma2a-zero-space-isolation-*` temporary fixtures. Both
  worktrees retain only protected untracked `.omo/**` evidence and have no tracked or staged changes.

## 2026-08-30 - Todo 14 control synchronization

- Retry policy needs a typed transient/permanent boundary before jitter is meaningful; otherwise authorization and validation failures consume retry budget.
- Aggregate timeout ownership belongs around the complete round, not individual dials, so queueing, Store work, retries, and spawned tasks share one deadline.
- Local control status is truthful when the actor records authenticated completed peers and clears that set on membership changes.
- Real-Iroh proof must actively dial without an existing connection and compare exact canonical signed bytes after persistence.

## 2026-08-30 - Todo 14 correction slice

- A boolean pending flag cannot preserve explicit peer intent or waiter relevance. A typed pending scope plus monotonically assigned round identity makes both properties deterministic.
- Accepted manifest, address, and relay changes should return machine-consumed change flags from the atomic apply boundary; those flags can feed one coalescing scheduler without source-text tests.
- A real-Iroh control scenario with two shared Spaces and one private Space proves both multi-Space convergence and shared-Space isolation in the same active-dial round.

## 2026-08-30 - Todo 14 final correction

- Aggregate codec limits must budget the production-encoded request or response baseline before dividing remaining bytes across pages; independently valid pages can otherwise exceed the enclosing frame.
- Targeted synchronization completion must require the requested authenticated peer in the round outcome, not merely a successful round that synchronized some other peer.
- Actor and store dispatchers remain easiest to audit as one ordered loop, while extracting command definitions and local publication helpers keeps the repository's pure-LOC policy intact.

## 2026-08-30 - Todo 14 verifier blocker correction

- Empty membership is not a valid shortcut for targeted synchronization; the normal zero-peer round path is required so the requested authenticated Endpoint remains the success criterion.
- Atomic batch APIs must independently enforce high-water rollback, fork, and replay semantics even when their main caller already stages validated artifacts.
- Preflight state must include prior candidates from the same batch, otherwise an advance followed by a rollback for one key can still commit the wrong final state.
- Live adversarial control coverage can authenticate a custom Iroh responder with the persisted peer identity and port, then assert malformed forwarded artifacts preserve exact durable high-water state.

## 2026-08-30 - Todo 14 second verifier correction

- Complete-chain batch persistence must compare stored and same-batch candidate genesis, generation, exact canonical replay, and prefix compatibility before any `replace_chain` call; transaction rollback after destructive writes is weaker than preflight-before-write reviewability.
- A complete `SpaceChain` already proves contiguous internal links, so batch high-water comparison can reuse the safe normal path's canonical-prefix semantics without duplicating manifest signature or transition validation.
- Live forwarding rejection proof is strongest when one authenticated Iroh responder sends canonical signed artifacts for each semantic class and the test compares the complete persisted control-space snapshot plus reconstructed lookup addresses before and after.

## 2026-08-30 - Todo 15 fast-forward integration

- Reconfirmed every promotion guard before mutation: root `master` and `HEAD` were exactly
  `b47344a4ccd5ddaac7e8ce68e573130ba40cfb6d`; `todo-15-relay-map` and its clean source worktree were
  exactly `bb4a8820b4120fc87795839162a0cbbdcc8a8676`; the merge base was the expected root base; and the
  source was a strict 15-commit descendant.
- The promoted range contained zero merge commits and no committed `.omo/**`, `target/**`, or
  `web/dist/**` path. Promotion used only
  `GIT_MASTER=1 git merge --ff-only todo-15-relay-map`, so root `master`, root `HEAD`, and the source
  branch now resolve exactly to the approved HEAD without a merge or rewritten commit.
- Integrated focused verification passed 13/13 tests: RelayMap 4/4, live address observation 5/5,
  Runtime reachability 3/3, and connected rogue-home filtering 1/1.
- The required locked aggregate command did not pass. `cargo run --locked -p xtask -- check` stopped
  at the LOC policy before aggregate tests because the approved HEAD has four files over the 250
  pure-LOC limit: `crates/ma2a-app/tests/control_sync_e2e/support.rs` (254),
  `crates/ma2a-net/src/endpoint.rs` (261), `crates/ma2a-runtime/src/actor.rs` (270), and
  `crates/ma2a-runtime/src/store.rs` (263). A direct locked `xtask check-loc` reproduced the same
  deterministic failure; no product source or approved history was modified to conceal it.
- `GIT_MASTER=1 git diff --check b47344a4ccd5ddaac7e8ce68e573130ba40cfb6d..HEAD` passed.
  Secret Guard found no staged files and scanned 347 tracked files with no secrets detected. This
  append-only record remains protected untracked `.omo/**` evidence and is not staged or committed.

## 2026-08-30 - Todo 15 LOC correction

- Preserve actor and store ordering by leaving each central dispatcher intact and extracting only
  shutdown finalization and protected identity initialization into private sibling modules.
- Endpoint bind options form a coherent public API unit that can move behind the existing endpoint
  re-export without changing Endpoint identity, relay mutation, publication, or observation paths.
- Control-sync deterministic clock and temporary-state ownership can move together while the fixture,
  artifact preparation, enrollment order, and adversarial assertions remain unchanged.
- The corrected pure-LOC counts are 218/41 for control-sync support/state, 222/43 for
  endpoint/options, 242/34 for actor/shutdown, and 213/54 for store/identity.
- Focused verification passed 22/22 tests, and the locked aggregate gate passed 300/300 Rust tests

## 2026-08-31 - Todo 21 Phase One E2E

- One exact `--test e2e` target can reuse accepted real-Iroh control-sync forwarding fixtures while adding source-driven A-F scenarios without production API expansion.
- Local Iroh test relays provide deterministic observed-home and relay-path proof without public Internet or N0 availability.
- Target-only lookup evidence must assert both inclusion of the target's signed relay and exclusion of unrelated Space relay metadata before the successful dial.
- Repetition evidence is most useful as uncaptured JSON lines plus nextest summaries; three completed runs each emitted A-F once and passed 14/14.
  plus 44/44 Web tests. Rust LSP diagnostics timed out; compiler, strict Clippy, nextest, rustfmt,
  LOC, and aggregate gates provide the fallback evidence.

## 2026-08-31 - Todo 16 Iroh connection management

- Iroh 1.1 supports live async relay insertion and removal, so relay reconfiguration can preserve the
  running Endpoint and persisted identity without reconstruction.
- Keep retries in one ownership layer: the generic connection manager performs finite transient-only
  recovery, while control sync uses its single-attempt seam because the control round already owns a
  bounded outer retry policy.
- `Connection::paths()` and `path_events()` are sufficient for bounded observational direct/relay
  telemetry. These observations must remain diagnostics rather than authorization or MA2A path scores.
- Exact-target requests should rely on the Endpoint's configured `SpaceAddressLookup`; explicit
  `EndpointAddr` requests belong only to internal callers that already hold validated target data.
- Bounded ownership and diagnostics remain reviewable with caps of 128 active connections, 128 remote
  telemetry keys, eight observations per remote, and 160-byte error details.
- Strict workspace, focused real-Iroh, full serial workspace, and locked aggregate verification passed;
  every new source file remains below 250 pure LOC.

## 2026-08-31 - Todo 17 Echo remediation

- A binary authorized/denied admission channel collapses Store failure into authorization denial. Carry `Result<(), EchoError>` across the pre-body channel so `Unavailable` remains distinguishable without reading bytes.
- The server deadline must enclose response-frame writing as well as admission, authorization, body read, decode, and dispatch; a paused Tokio test proves the phases share one ten-second budget.
- Real-Iroh concurrency proof needs an observable permit-ownership barrier rather than sleeps. An RAII active-stream metric lets the test wait for exactly 16 admitted streams, observe the 17th rejection, release one stream, and observe a successful replacement.
- Failure auditing is possible only when a canonical request ID is known. Outbound failures and decoded target mismatches are auditable; pre-body denial, oversized wire input, and undecodable malformed input intentionally produce no record.
- Extracting membership observation into a private actor submodule preserved dispatcher ordering and reduced `actor.rs` from 269 to 247 policy LOC.

## 2026-08-31 - Todo 17 independent-verification correction

- A server timeout cannot consume the complete stream deadline if the protocol requires a typed timeout frame. Reserve bounded write time inside the aggregate budget, convert processing `Elapsed` into the stable wire error, and apply the original absolute deadline to the write.
- Global concurrency behavior is proved at the handler boundary by pre-acquiring the production limiter permits and observing status `5` over one real Iroh stream; constructing 129 network peers is unnecessary.
- Aborting an internal oneshot task proves only channel closure. Runtime shutdown may race typed cancellation framing with Endpoint closure, so evidence must promise transport closure rather than guaranteed status `6` delivery.

## 2026-08-31 - Todo 17 fast-forward integration

- Exact-source promotion is safe when root is first pinned to the requested base, the source ref and worktree are independently checked, and the range is audited for ancestry, commit count, merge commits, forbidden paths, whitespace, and secrets before the sole `GIT_MASTER=1 git merge --ff-only` mutation.
- Integrated Echo verification passed 5/5 focused `ma2a-net` handler tests, 6/6 real-Iroh app E2E tests, and the locked aggregate gate with 327/327 Rust and 44/44 Web tests. Existing duplicate-version and jsdom canvas notices remained non-fatal.
- Post-promotion Secret Guard scans remained clean, the source worktree stayed unchanged and clean, and root retained only protected untracked `.omo/**` evidence. An unrelated Todo 16 credential-daemon process was not disturbed.

## 2026-08-31 - Todo 18 snapshot and SSE correction

- Snapshot revisions must come from the same durable Store snapshot used to construct the payload; copying an actor-local revision into the response envelope can expose an impossible mixed revision.
- Effective relay truth needs separate authorized candidates and observed connected private/public relay classes. Configured candidates are not evidence of connectivity, and a connected public relay must not satisfy private-relay status.
- Axum SSE keep-alive comments provide an unrevisioned heartbeat without entering the bounded application queue; dropping the body closes the sender and should cancel polling immediately.
- Frontend recovery is simpler when strict codecs own the untrusted browser boundary and one coordinator owns replacement-snapshot serialization, retries, invalidation, and stale-snapshot rejection.
- Live loopback QA proved the complete operational contract: authenticated snapshot, stale-baseline resync, current-baseline comment heartbeat, unauthenticated rejection, and authenticated service-unavailable behavior after daemon shutdown.
- Todo 18 was delivered as 15 dependency-ordered Conventional Commits, keeping Store truth, Runtime projection, API framing, Web transport, frontend recovery, and documentation independently reviewable.

## 2026-08-31 - Todo 21 exact-target correction

- Exact release-gate composition must include the required behaviors in the requested test binary; equivalent lower-level tests in other binaries do not satisfy an exact `--test e2e` contract.
- Restart persistence evidence is regression-sensitive only when it compares exact typed high-water artifacts, including content hashes, rather than a repository-wide revision inequality.
- A repeated evidence runner should validate exact test totals, exact unique scenario schemas, per-file checksums, and exact artifact counts. Provenance must be regenerated only after the final source commits exist.
- Final committed evidence at `578260d3306f549891074854cefba3312439b1cc` passed three consecutive 46-test runs with 13 structured records per run.

## 2026-08-31 - Todo 19 bounded Nextest concurrency delivery

## 2026-09-01 - Todo 22 delivery evidence receipt

- Reverified exact product commit `4cc94fecd40a0ac28460b84b8bab6bc1955c0acc` on native `x86_64-unknown-linux-gnu`; no product, config, script, or documentation defect was found, so no follow-up commit was required.
- Required release commands passed with exit code 0: five Todo 22 xtask tests, `check-release`, `check-support`, `check-pins`, native `cargo xtask dist`, archive/checksum validation, dependency-license generation, deterministic SPDX generation, metadata validation, extracted-binary clean-room smoke, and negative tamper/missing-asset/incomplete-SBOM probes.
- Native cargo-dist output was `ma2a-app-x86_64-unknown-linux-gnu.tar.xz`, SHA-256 `f1baf0309e2589e2ecb15174aabe89452e4d9b8ab08479e5bd46917f084fe4e6`, with one top-level directory containing exactly `ma2a`, `README.md`, `quickstart.md`, `LICENSE-MIT`, `LICENSE-APACHE`, and `NOTICE`.
- Created sanitized evidence at `.omo/evidence/task-22-ma2a-phase-0-1.zip` with sidecar `.omo/evidence/task-22-ma2a-phase-0-1.zip.sha256`; the evidence zip SHA-256 is `8bf9e84a0abca24ff4d1741d126c36432201170c75ba4909d50f44e62e7442f9`.
- Secret Guard path scans passed before packaging. Evidence contains transcripts, concise metadata summaries, matrix/workflow audits, exact archive inventory, Git audit, and cleanup receipt, but excludes the generated executable, credential values, cookies, tokens, CSRF values, invitation material, private keys, runtime databases, and private temporary paths.
- Zip verification passed the external SHA-256 sidecar, `unzip -t`, direct reads of the verification summary and evidence index, and complete internal `MANIFEST.sha256` validation after disposable extraction.

## 2026-09-01 - Final-wave F4 CLI scope correction

- Removing the hidden singular `ui session revoke-all` enum variant and its duplicate workflow arm leaves the canonical plural `ui sessions revoke-all` process path unchanged while making the singular spelling fail at the CLI boundary.
- The app's process-level parser normalizes Clap parse failures to `unknown command; run ma2a --help`; the regression asserts this user-facing error and exit code 2 rather than raw Clap formatting.
- Cleanup removed `target/distrib`, the Task 22 release-inspection daemon and private state directory, disposable extraction directories, and the evidence staging directory. Tracked files remained clean and `.omo/**` remained uncommitted.

- Committed the pre-verified two-file concurrency correction as `test(workspace): bound integration concurrency`; both Nextest profiles use four test threads and the `ma2a-app` process-heavy group is capped at four threads.
- The staged regression asserts the global/profile values and application test-group maximum remain exactly `4`; no serial execution workaround was introduced.
- The receipt is append-only and remains outside Git staging with the rest of `.omo/**`.

## 2026-09-01 - Todo 22 tracked smoke correction

- The tracked extracted-archive smoke now obtains its loopback URL through the canonical `ma2a ui open` command; the complete native Linux archive smoke passed through offline assets, login, authenticated snapshot, logout, and original-cookie revocation.
- The existing release failure probe now guards the tracked smoke source against both omission of the canonical command and reintroduction of the removed `ma2a web` invocation, while preserving tamper, missing-asset, external-reference, and incomplete-SBOM assertions.
- A fresh native Linux archive passed archive/checksum validation, metadata validation, clean-room smoke, and all release failure probes after the correction.

## 2026-08-31 - Todo 21 integration precondition finding

- The requested Todo 21 candidate ref/worktree is present and resolves exactly to
  `578260d3306f549891074854cefba3312439b1cc`, but it is not a descendant of the current Todo 19
  `master` at `f40b2608fd3e03c7f00efa281b2ee66c6ed33692`. Their merge base is Todo 18
  `ead410f8f37a6c20e06b45180e956ca9d652d6db`; the candidate has 20 linear commits from that base.
- The candidate range itself has zero merge commits and no committed `.omo/**`, `target/**`, or
  `web/dist/**` paths, but it omits the accepted Todo 19 commit range. The main worktree's two
  intended Todo 19 concurrency edits remain uncommitted and were not altered.
- Because strict fast-forward ancestry is false, no `GIT_MASTER=1 git merge --ff-only` was run and
  no ref, source file, Todo 20 state, or protected `.omo/**` evidence was changed. A valid integration
  requires a corrected candidate based on `f40b260...` or explicit orchestration direction for a
  non-fast-forward integration; this task did not authorize either alternative.

## 2026-08-31 - Todo 19 fast-forward integration

- Reconfirmed the exact promotion guards before mutation: root was on `master` at `ead410f8f37a6c20e06b45180e956ca9d652d6db` with no tracked or staged changes and only protected untracked `.omo/**`; the clean source worktree and `todo-19-cli-workflows` both resolved to `f40b2608fd3e03c7f00efa281b2ee66c6ed33692`.
- The accepted source was a strict 45-commit linear descendant of root with merge base `ead410f8f37a6c20e06b45180e956ca9d652d6db`, zero merge commits, and no committed `.omo/**`, `target/**`, or `web/dist/**` path. Promotion used only `GIT_MASTER=1 git merge --ff-only todo-19-cli-workflows`; no rebase, squash, amend, compatibility commit, or source edit occurred.
- Final root `master`, root `HEAD`, and the source branch resolve exactly to `f40b2608fd3e03c7f00efa281b2ee66c6ed33692`, preserving all 45 reviewed commits in order. `GIT_MASTER=1 git diff --check ead410f8f37a6c20e06b45180e956ca9d652d6db..HEAD` passed.
- Focused candidate verification passed rustfmt, 25/25 endpoint CLI workflow/security tests, the dedicated 11/11 secret-argv tests, and 86/86 Runtime tests. Integrated strict workspace Clippy and `cargo run --locked -p xtask -- check-loc` passed.
- Two default-concurrency integrated `cargo run --locked -p xtask -- check` attempts exposed resource-sensitive process-test failures (`private IPC frame is invalid`) and the known long enrollment case exceeded nextest's 120-second timeout. Every affected CLI test passed immediately in exact isolation, and the unchanged enrollment case passed alone in 104.352 seconds. Failed-run root test daemons were shut down without touching pre-existing Todo 16 or candidate-worktree daemons.
- The constrained authoritative rerun `NEXTEST_TEST_THREADS=1 cargo run --locked -p xtask -- check` passed end to end: 368/368 Rust tests and 54/54 Web tests, formatting, strict Clippy, LOC, dependency advisories/bans/licenses/sources, unused-dependency analysis, Biome, strict TypeScript, and frontend tests. Existing duplicate-version warnings and jsdom canvas notices remained non-fatal.
- Shared-root LSP diagnostics were attempted for App secret argv, Runtime snapshot, and Store revision files; all timed out at the known 30-second daemon/MCP limits. Compiler, rustfmt, strict Clippy, focused nextest, LOC, and the aggregate gate provide fallback diagnostics.
- Final root has no tracked or staged changes and retains only protected untracked `.omo/**` evidence, including this append-only integration record. No root-owned Todo 19 test daemon remains.

## 2026-09-01 - Todo 21 rebase and fast-forward integration

- Rebased the 20-commit Todo 21 range from old base `ead410f8f37a6c20e06b45180e956ca9d652d6db` onto Todo 19 `master` at `f40b2608fd3e03c7f00efa281b2ee66c6ed33692`; range-diff preserved every commit in order and the rebased tip is `36d6be7ede40b6e3141da2bb7d148d8a644ed260`.
- The sole `Cargo.lock` conflict retained Todo 19 CLI/Windows dependencies while adding Todo 21 dependencies. Promotion used `GIT_MASTER=1 git merge --ff-only todo-21-phase-one-e2e`; `HEAD`, `master`, and the source branch remain identical with zero merge commits.
- The integrated recovery E2E exposed a stale test contract, not a production recovery failure: `status` defaults to human output while the test parsed JSON, and the current JSON response type is `snapshot`. Adding `--json` and asserting `snapshot` made the existing failing-first test pass while the stale endpoint was replaced by a mode-0600 Unix socket.
- Three unchanged isolated E2E runs passed 46/46 each. Two unchanged locked aggregate runs passed all Rust, dependency-policy, frontend lint/type/build, and 54/54 frontend-test gates. Rustfmt, strict workspace Clippy, LOC, diff, history, forbidden-path, and Secret Guard audits also passed.
- Todo 19 bounded Nextest concurrency edits remain staged and verified. Todo 20 and Todo 22/release files were not modified. Main-path Rust LSP diagnostics timed out twice at the known 30-second daemon limit; compiler, Clippy, rustfmt, nextest, and aggregate checks provide fallback diagnostics.

## 2026-09-01 - Todo 20 authenticated console integration

- Fast-forwarded the reviewed `todo-20-authenticated-console` history onto `master`; the integrated tip is `b2567973a124d146420e4bd379784e09b69eca16`, preserving the candidate's dependency-ordered commits without a merge commit or history rewrite.
- Web mutation payloads now match the Rust codecs exactly. Invitation generation accepts `space_id`, `ttl_ms`, and an owner-selected `output_path`; Private Relay configuration explicitly distinguishes native TLS certificate/key paths from external termination with both paths set to `null`.
- The embedded authenticated console uses authoritative Runtime snapshots after mutations and uncertainty, keeps credentials and bearer material out of browser storage, and exposes complete Space, relay, Echo, session, and settings operations without returning invitation contents to JavaScript.
- Embedded-binary browser QA covered authenticated login/logout, snapshot refresh, Space creation, owner-only invitation-file generation, external-termination Private Relay configuration, and every authenticated route at 375, 768, and 1280 CSS pixels. The generated invitation was a nonempty regular file with mode `0600`; no document-level horizontal overflow, clipped route, detached label, blank capture, or console failure remained.
- Focused and aggregate verification passed Biome, strict TypeScript, all 62 Web tests, production Web build, rustfmt, strict workspace Clippy, LOC policy, Runtime Web mutation/API contract tests, and the locked aggregate gate with 417 Rust tests. Secret Guard tracked/staged scans and ignore coverage passed.
- The pre-existing staged Todo 19 Nextest controls and unstaged Todo 21 recovery correction were preserved byte-for-byte during promotion and verification. Todo 22 release workflow, packaging, provenance, and platform documentation were not modified.

## 2026-09-01 - Todo 20 embedded UI correction closeout

- Splitting Linux process/listener discovery into `embedded_ui/support.rs` preserved the production-binary acceptance behavior while reducing the scenario file to 223 pure LOC; the support module is 50 pure LOC.
- The exact locked `embedded_ui` target passed 1/1, strict target Clippy and rustfmt passed, and the locked aggregate gate completed successfully with the existing duplicate-dependency warnings and JSDOM canvas notices only.
- Secret Guard found no secrets in staged or tracked files or the new embedded UI helper directory, and all common sensitive-file patterns remain covered by `.gitignore`.

## 2026-09-01 - Todo 20 bounded recovery and forced sign-out correction

- Controller-owned snapshot recovery remains authoritative when retries are bounded to 250, 1000, and 4000 milliseconds, retain the last known snapshot as offline, replace it only after a successful fetch, and keep exactly one EventSource subscription.
- Revoke-all is a single authenticated mutation: the successful response expires both session cookies, while the browser clears local session state and navigates directly to `/login` without issuing a second logout request.
- Fresh embedded-binary browser QA captured all six authenticated routes at 1280x900 and 375x812. Geometry checks found no document-level horizontal overflow, visual inspection found no overlap or malformed controls, and the relay select inherited the shared 16px system font.
- Browser QA observed zero console warnings/errors and only loopback requests. Before revoke-all, browser storage was empty and only the CSRF cookie was script-readable; afterward all cookies and browser storage were empty at `/login`.

## 2026-09-01 - Todo 20 correction commit receipt

- Committed the verified correction as `c490e1ea8c6aca9841c51da00a0ec1dc730c41de` with `fix(web): recover authenticated console sessions`; the commit contains exactly the 17 Todo 20 correction paths.
- The two staged Todo 19 files and the unstaged Todo 21 recovery plus `Cargo.lock` diffs were preserved byte-identically; `.omo/**` remains uncommitted.
- Exact staged correction Secret Guard scan passed with no findings before commit; commit diff check passed.

## 2026-09-01 - Todo 21 recovery E2E contract correction

- Committed `crates/ma2a-app/tests/e2e/recovery.rs` as `9c9749b` (`test(e2e): align recovery status contract`): the stale-socket process probe now requests `status --json` and asserts the current typed `snapshot` response.
- A parsed TOML comparison proved `Cargo.lock` had identical lock version, all 473 package records, and the same 24 `ma2a-app` dependency entries; only `clap`/`axum` and `windows-sys`/`tower` order differed. The locked focused test passed without requiring that ordering, so only `Cargo.lock` was restored to HEAD and excluded from the commit.
- Focused verification passed 1/1 with `cargo nextest run --locked -p ma2a-app --test e2e --profile ci recovery::stale_ipc_endpoint_is_recovered_by_a_real_daemon_process`; staged Secret Guard and `git diff --check` passed. This receipt remains uncommitted under protected `.omo/**`.

## 2026-09-01 - Todo 22 release correction

- Release policy must compose the exact six-target support validator before deriving the supported subset; otherwise removing a deferred architecture leaves cargo-dist equality unchanged and silently passes.
- Deleting each required target independently is the direct regression shape: all three deferred-target deletions failed before the fix, while supported-target deletions already failed through config equality.
- The extracted archive smoke now traverses actual browser-loading references from HTML, CSS, and JavaScript, proving every dependency is served from loopback without falsely rejecting namespace or documentation URL constants embedded by libraries.
- Owner-only request, cookie, login-response, and curl-config files support real login, authenticated snapshot, logout, and post-logout rejection without exposing credentials or tokens in argv, URLs, or sanitized logs.
- A same-length embedded favicon mutation produced a checksum-valid archive that passed archive validation and failed only at the external-reference smoke gate, proving the new negative probe distinguishes integrity from offline-runtime behavior.
- Corrected evidence includes the native archive and checksum sidecar so digest and inventory claims are independently recomputable from included bytes.

## 2026-09-01 - Todo 22 ws/wss fixture closeout

- Production already rejected both `ws://` and `wss://`; the remaining acceptance gap was deterministic fixture coverage. Adding a second literal WebSocket construction and an exact `network\tws://socket.invalid/plain` assertion preserved the existing secure-scheme assertion and required no scanner behavior change.
- A disposable red proof added only the new expected `ws://` output and exited 1 against the old fixture; the committed input/assertion pair then emitted and asserted both schemes successfully.
- Commit `c8676ea48a618c7ae1b377ad0cc4508fb8f9f5df` contains only the two-line fixture correction in `scripts/test-runtime-references.sh`.

## 2026-09-01 - F4 CLI scope correction

## 2026-09-01 - F2 durable local mutation replay

- Durable replay must store the exact encoded success envelope, not only the typed result and revision, because replayed graceful shutdown must preserve the original server-exit decision and clients require byte-identical responses.
- A persistent pending reservation closes the crash window between request admission and replay completion: after restart, the Runtime fails identical retries closed with `unavailable` instead of risking duplicate local or external side effects.
- SQLite triggers provide deterministic retention rollback tests: rejecting the required oldest-row deletion proves response completion and eviction remain one transaction while the pending reservation survives unchanged.

- Removed the accidental public `ma2a web` command and its dedicated URL-printing module; the authenticated embedded Web console remains owned by Runtime and the canonical `ma2a ui open` workflow.
- Added process-level help coverage proving top-level `web` absence and nested `ui open` presence. Focused CLI tests, `ui open` E2E, rustfmt, app Clippy, build, and direct binary help checks passed.
- The locked aggregate gate was attempted but is blocked by unrelated concurrent Store test changes importing APIs not present in this worktree; those changes and Cargo.lock churn were preserved outside this task.

## 2026-09-01T14:02:49+10:00 - F2 inbound control admission remediation

- Inbound `ma2a/control/1` now acquires shared global and authenticated-peer RAII capacity before Runtime admission, and Runtime authorizes shared-Space membership before transport reads or allocates the bounded request body.
- A two-phase `ControlCall`/`ControlAuthorizedCall` API makes the ordering structural: only an authorized call exposes the body receiver and single-use response capability.
- Production limits are 16 concurrent inbound control streams globally and two per authenticated peer. Permits remain owned through response completion or stream exit and release deterministically on every path.
- Runtime handles admitted bodies in a bounded `JoinSet` task path so the actor can continue servicing lifecycle and other protocol work; the existing response-time Store authorization remains as a revocation-safe second check.
- Real-Iroh transport coverage proves unauthorized rejection before body read, same-peer saturation before a maximum-size body is accepted, permit release after rejection and completion, and deterministic global saturation using the production limiter.
- Iroh does not surface a newly opened bidirectional stream to the peer until at least one byte is sent. Admission tests therefore send one probe byte before awaiting `ControlCall`; Runtime authorization still completes before the handler reads that byte.
- Final source audit found no stale pre-authorization `ControlCall::request` or direct `ControlCall::respond` use. `Cargo.lock` contains no remaining diff, and `.omo/**` evidence remains untracked and excluded from staging.

## 2026-09-01T15:30:00+10:00 - Todo 22 final release rebuild

- Rebuilt the native Linux x86_64 archive at exact product commit `cf39471f22b0353b048bc8f5cb4a0066fb1f85e1`, including durable replay through `d8c55cd`, pre-body control authorization/admission `d6ce465`, removal of `ma2a web` at `c423d0b`, and the corrected Echo CLI contract at `cf39471`.
- The final archive contains exactly the binary, README, quickstart, two licenses, and NOTICE. Direct extracted-binary QA proved `web` absent, `ui open` present, and endpoint + text/stdin with optional JSON accepted by clap without requiring a remote Echo success.
- Archive SHA-256 is `a623485543cc363c69ecf41288ef956beaf2a84d18f624071d953a8e73bcd882`; SPDX SHA-256 is `53a5f6a5a15b58c37ec295eaa814f73981cd10c32664c0a96a58ba538971fdad` with 478 packages.

## 2026-09-01 - Todo 22 final evidence correction rebuild

- Rebuilt the native Linux x86_64 archive from exact final HEAD `9afeb98b2c5c3e72d996985fc6fbe9544b5242b9`; archive SHA-256 is `2635fe38b6ddd528eac5bedf766d3a0732d36e08ae097748a9309ece31f2245d`.
- The final SPDX SBOM SHA-256 is `bb053e98c7a7f41bce0d34eb7b682f2fc1f0b133a3ceeabeec072e9f709d1ff5` with 478 packages. The corrected evidence manifest validates all nine evidence files.
- Direct tracked clean-room smoke and release failure probes passed. Extracted binary probes prove `web` is absent, `ui open` is present, and corrected Endpoint-addressed Echo combinations parse without usage errors.
- Rebuilt `.omo/evidence/task-22-ma2a-phase-0-1.zip` from the finalized correction directory. Its SHA-256 is `f246234ce5b19a268bd4cbf5c23a85d9e52bd05c5cdfe762cb44397e7fa1970d`; clean extraction, ZIP integrity, manifest, archive sidecar, and inventory checks passed.

## 2026-09-01 - Todo 22 checksum formatting correction

- Normalized the included archive sidecar to exactly one POSIX checksum line while preserving archive SHA-256 `2635fe38b6ddd528eac5bedf766d3a0732d36e08ae097748a9309ece31f2245d`.
- Regenerated the evidence manifest and outer ZIP; all archive, manifest, and ZIP checksum layers now exit 0 with zero stderr warnings. The rebuilt ZIP SHA-256 is `72375bb5ba1d0b6fce1f0b502e3063fff36634287d1a529a736731df783b673f`.


## 2026-09-01T19:09:44+10:00 - Independent F4 CLI alias verification

- Independently verified exact commit `c1165ecb48ea3deba3617a9b688667a2fa51229d` with exact parent `9afeb98b2c5c3e72d996985fc6fbe9544b5242b9`; the commit changes only `crates/ma2a-app/src/cli.rs`, `crates/ma2a-app/src/commands/workflows.rs`, and `crates/ma2a-app/tests/credential_daemon.rs` (19 insertions, 10 deletions), with no `Cargo.lock`, docs, workflow, release, `.omo/**`, `target/**`, or `web/dist/**` tracked scope.
- Code and diff inspection confirmed the production change only removes hidden `UiCommand::Session` and its duplicate `run_ui` match alternative. Canonical `UiCommand::Sessions::RevokeAll` still calls the unchanged `credential_command::run_revoke_all` workflow; no Runtime session API, command codec, store, daemon, or public API changed.
- Fresh current-HEAD binary behavior was exact: singular `ui session revoke-all` exited 2, wrote zero stdout bytes, wrote exactly `unknown command; run ma2a --help\n` to stderr, and created zero state-directory entries; plural `ui sessions revoke-all` exited 0, wrote exactly `All Web sessions revoked\n`, wrote zero stderr bytes, and created/held the daemon state and lock path.
- A separately built parent binary accepted singular `ui session revoke-all` with exit 0 and the revoke message, while the reviewed binary rejected it, proving the regression is sensitive to this commit rather than a stale artifact. Focused Nextest passed 2/2; rustfmt and strict all-target/all-feature `ma2a-app` Clippy passed.
- LSP diagnostics were attempted independently for all three changed Rust files. `cli.rs` timed out after the daemon stated 30000 ms; `workflows.rs` and `credential_daemon.rs` each returned MCP error `-32001: Request timed out`. Compiler, Clippy, rustfmt, process tests, and direct binary probes provide fallback diagnostics.
- Residual-diff checks found no TODO/FIXME/HACK, lint suppression, `unsafe`, accidental public API declaration, forbidden path, whitespace error, tracked/staged worktree change, or `Cargo.lock` change. Verdict: `AdversarialVerify: confirmed`; `VERDICT: APPROVE` on native Linux x86_64 only.

## 2026-09-01T20:05:10+10:00 - Todo 22 exact-final-HEAD evidence rebuild

- Rebuilt the native Linux x86_64 release archive from exact product HEAD `c1165ecb48ea3deba3617a9b688667a2fa51229d`; archive SHA-256 is `7152995b6897b4e827e63f178b6af636d3abaa6db11e22c009ca8ae055fd53df`.
- Generated and validated SPDX 2.3 metadata with SHA-256 `7a1af336bdbe53822b79c3b944b1362e0ee524dfa7706244ab4a1ef3af9becd1` and 478 packages. The sanitized dependency/license report SHA-256 is `413482220500445e937b940da2dbabe0425a82bc5d6fd7d441378c74b21a7b28`.
- All 19 xtask tests, release/support/pin/LOC policy gates, runtime-reference fixtures, archive validation, metadata validation, tracked authenticated clean-room smoke, and integrity/missing-asset/external-reference/incomplete-SBOM failure probes passed. Focused F4 process tests passed 1/1 for singular rejection and 1/1 for canonical plural revoke-all.
- Direct extracted-binary QA proved top-level `web` absent, `ui open` present, all Endpoint-addressed Echo combinations parse, singular `ui session revoke-all` exits 2 with exact normalized stderr and zero state entries, and plural `ui sessions revoke-all` exits 0 with the exact revoke message before dedicated daemon cleanup.
- Rebuilt `.omo/evidence/task-22-ma2a-phase-0-1.zip`; ZIP SHA-256 is `6721c1ff5c9055c2c8392c560171846bcbb90e0c3748fd864c2aca1cea72f4a2`. Disposable extraction verified ZIP integrity, the external ZIP sidecar, all nine nested manifest entries, the archive sidecar, archive inventory, SPDX binding, and zero checksum stderr warnings.
- Secret Guard found no evidence secrets, all common sensitive patterns remain ignored, and generated local workspace paths were normalized to `/workspace/ma2a/`. Cargo's known dependency-order-only lockfile churn was restored to exact HEAD after each generation phase.

## 2026-09-01T21:08:06+10:00 - Final F3 evidence refresh

- Final real manual QA is bound to exact product commit `c1165ecb48ea3deba3617a9b688667a2fa51229d` and extracted native Linux x86_64 archive SHA-256 `7152995b6897b4e827e63f178b6af636d3abaa6db11e22c009ca8ae055fd53df`.
- Direct extracted-binary probes proved top-level `web` and singular `ui session revoke-all` both exit 2 with exact normalized stderr, zero stdout, and zero state creation; canonical plural `ui sessions revoke-all` reaches the daemon control path and exits 0.
- Fresh browser QA covered all six authenticated routes at `1280x900` and `390x844`, two-context revoke-all, ordinary logout, empty Web Storage, strict cookie properties, deliberate offline state, and restart recovery with Endpoint, Space, and host-scoped session continuity.
- The exact locked `ma2a-app --test e2e` target passed 46/46 with 0 failed and 0 skipped. Manual self-target Echo returned typed `unavailable`; successful peer Echo remains attributed only to the executed real-Iroh E2E case.
- A first independent visual pass found three desktop captures at the browser tool's default `780x493`; recapturing login, offline, and recovered states at `1280x900` resolved the evidence-only inconsistency without a product change.

## 2026-09-01 - Final F1 plan-compliance receipt refresh

- Replaced the stale F1 rejection with the latest original reviewer approval bound to exact final HEAD `c1165ecb48ea3deba3617a9b688667a2fa51229d`.
- The refreshed receipt records final archive and evidence hashes, fresh aggregate, frontend, E2E, and clean-room smoke results, the singular session-alias rejection, and the native Linux x86_64 execution boundary.
- No tracked product file, plan, or Boulder state changed. `Cargo.lock` remains identical to HEAD and only protected untracked `.omo/` evidence is present.
