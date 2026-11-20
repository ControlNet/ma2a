# Round-5 review: what was fixed, and why the rest waits

This closes out `2026-11-21-review-round-5.md`, the Codex report on baseline `8ded423`. Each
finding was checked against the code before it was fixed. Fixes landed one commit per finding,
each with a test that fails without it. They run from `ff98ab0` to `5dcdd7e` on `master`.

## Status

| Finding | Status | Commit | Notes |
| --- | --- | --- | --- |
| F01 retried departure removes a rejoined member | fixed | `5dcdd7e` | Schema 11 adds `departure_receipts(space, sender, request_id) → generation`, written in the transaction that removes. A retry replays the chain through that generation and commits nothing. The test covers two membership cycles. |
| F02 history-full refusal stops the actor | fixed | `ff98ab0` | `RuntimeError::is_request_refusal` covers `SpaceHistoryFull` and `SpaceOwnerCannotBeRemoved`, and `absorb_background` logs them instead of returning them. |
| F03 reserve cannot revoke every member | fixed | `2edeaa7` | Every change except a removal must keep `255 - generation ≥ non-owner members` and `revocations + non-owner members ≤ 64`. A removal (it revokes and adds no member) is always allowed, so an older history that already breaks the rule can still shrink. The refusal is `SpaceHistoryFull`, which is request-scoped since F02. |
| F04 split peer exceeds its stream limit and reports partial success | fixed | `cd3b396` | `DialQueue` limits dials per peer and records failures; a peer counts as synchronized only when every one of its shards succeeded. |
| F05 mixed-length manifests skip a generation | fixed | `782a969` | `add_contiguous` stops at the first manifest that does not fit. |
| F06 no re-bootstrap when every known address expired | deferred | — | This is a feature: explicitly importing a fresh signed address. The docs already describe the limitation accurately, and nothing is unsafe meanwhile. |
| F07 + F26 backup published in place, readable by others | fixed | `66e40c1` | The backup is staged under a random owner-only name, then published with a hard link that never replaces. |
| F08 Windows ACL predicate accepts partial masks | deferred | — | Windows-only. It cannot be compiled or tested natively here. |
| F09 staging names reserved in one letter case only | fixed | `caced78` | |
| F10 upload sweep proves nothing but the path | deferred | — | Staging names are `.ma2a-upload-<128-bit id>.part`, generated per request, and since F09 the Files service reserves them in every letter case. A collision needs that exact name to already exist. Proving ownership needs another schema change to store a file identity, and Windows has no portable inode. |
| F11 download helpers claimed by name alone | fixed | `5026488` | Helpers now live in an owner-only `.ma2a-downloads/` beside the destination, named by BLAKE3 of the lowercased file name. All three acceptance cases are covered: an unrelated empty file, a file named like an old helper, and two downloads whose names overlap. Legacy `NAME.ma2a-part*` files are never touched. |
| F12 cut-short staged sidecar refused | fixed | `84a38d3` | |
| F13 unbound Process release lost after a transient error | fixed | `569266a` | Four attempts with backoff. There is **no transient-fault injection test**: the `MA2A_TEST_AGENT_FAULT` hooks only crash. Verified by regression (agent unit tests and `agent_e2e`). |
| F14 Windows custom A2A token path inherits parent ACL | deferred | — | Windows-only, same as F08. The docs already require a private `--token-file` directory. |
| F15 two A2A adapters share one token file | fixed | `44e75be` | An OS lock on `TOKEN_FILE.lock` is held for the whole run. A second adapter gets a usage error before it writes anything. |
| F16 hosted admission mixes two registration generations | fixed | `238e1b1` | Admission holds the lifecycle lock from the registration read until the slot is taken. |
| F17 oversized pre-upgrade exposure list | **partial** | `63569d9` | The list now answers `conflict` with a remedy (`ma2a service unexpose`) instead of failing to encode. Paging the list would change the exact-version local API. The oversized state could only come from an unreleased source build between 2026-10-01 and `8321d0c`. |
| F18 E2E script deletes an operator directory | fixed | `e94e6bc` | |
| F19 SDK retries a version mismatch forever | fixed | `be5fcd0`, `b345a8a` | Lasting errors are returned. The follow-up matters: a busy daemon answers the handshake with a version-1 `unavailable` envelope, which `validate_handshake` used to report as `VersionMismatch`. The service SDK would then have stopped on a merely busy Runtime, and `ma2a status` would have called it incompatible. |
| F20 stopped web controller clears the new session | fixed | `4fe5d92` | |
| F21 telemetry evicts by key order | fixed | `c04d5ff` | Each remote carries a last-observation sequence, so ties and clock steps cannot matter. |
| F22 pin check accepts version unions | fixed | `6439b2a` | `xtask/src/semver.rs` must read the whole requirement as one SemVer 2.0.0 version. |
| F23 per-target SBOM lists every platform's crates | deferred | — | No release has been cut and none will be for now, so `release.yml` never runs. Fix it before the first tag. |
| F24 requester ahead of the chain is rejected | fixed | `f0645cb` | |
| F25 caller SDK cannot send control frames | fixed | `f0e2949` | `CallStream::control` is checked against a duplex stream at the 16 KiB bound. Neither end-to-end test path uses the Caller SDK. |

## Lessons

- **Handshake classification is shared.** `validate_handshake` feeds the CLI daemon-state probe,
  every `LocalApiClient::call`, and the service SDK. Before treating `VersionMismatch` as lasting
  anywhere, check what a busy or draining daemon of the *same* version answers.
- **Test keys can collide with fixtures.** In `crates/ma2a-store/tests/space_history_limit.rs`,
  the `OWNER` constant is the Ed25519 public key of seed `[65; 32]`. Generating members from
  `SpaceAuthoritySecret::from_bytes([i; 32])` therefore turns index 65 into the owner.
- **Schema bumps break rollback fixtures.** Tests that rebuild an older schema by dropping tables
  (`mutation_replay.rs`, `processes.rs`) must also drop each newer table and its
  `schema_migrations` row.
- **Cleanup races with the private download directory.** The directory is removed when it
  empties. A concurrent download can lose it between `prepare` and opening its lock, so the
  lease retries after re-creating it (`lease::locked`).

## Known flakes

These are unrelated to these changes:

- `echo_e2e` can fail with "Address already in use".
- `ipc::server::tests::lifecycle::a_lifecycle_call_is_answered_when_every_business_slot_is_taken`
  can fail under full load and passes when rerun.

## Verification at `5dcdd7e`

- `cargo build --workspace --bins && cargo run --locked -p xtask -- check`: exit 0, 965 Rust
  tests passed (nextest), cargo-deny, pins, and LOC policy clean.
- `(cd web && bun run check && bun run test)`: 151 tests passed.
