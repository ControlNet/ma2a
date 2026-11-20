# Round-6 review: what was fixed, and why the rest waits

This closes out `2026-11-22-review-round-6.md`, the Codex report on baseline `1d96672`. Each
finding was checked against the code before it was fixed. Fixes landed one commit per finding or
per tightly coupled group, each with a test that fails without it where one can run on Linux.
They run from `6f6d442` to `87875a6` on `master`.

## Status

| Finding | Status | Commit | Notes |
| --- | --- | --- | --- |
| F01 retried first departure gets the second removal | fixed | `6f6d442` | The actor no longer checks membership first. The Store backend looks up the receipt before membership. A request with no receipt from a non-member (including one answered before schema 11) gets the latest removal. A non-member with no request id is `SpaceNotFound`, which the actor answers with nothing. The test covers two departure cycles. |
| F02 swap passes as a removal | fixed | `47cdcc1` | A removal must revoke, and its non-owner members must be a strict subset of the previous set (set comparison, not counts). The test covers a same-count swap (refused) and a two-member batch removal (allowed). |
| F03 full Space stops the owner's actor | fixed | `0014c4b` | New `StoreError::SpaceMembersFull`, checked before signing against `SpacePolicyV1::maximum_members()` (now public). It is a request-scoped refusal in `is_request_refusal`. The invitation stays unused: a second attempt with another request is refused the same way. Owned Spaces always use the default policy (64), so a lower limit cannot be built through public APIs; the test covers the default limit. |
| F04 non-authority member stops on a departure request | fixed | `6f6d442` | `departure::answers_departure(chain, local, departing)`: only the genesis owner answers, never for itself. It is pure and unit-tested. |
| F05 global waiter succeeds when every peer was partial | fixed | `ddd4756` | `ControlRoundOutcome::incomplete` is set when peers were dialed but none completed every request. The actor keeps the committed changes and revision, but fails the waiter and records the round as `failed`. A truly empty round is unchanged. |
| F06 leading side re-pushes the same prefix | deferred | — | It needs either a wire change (the lagging side reporting how far it accepted) or a scheduled reverse pull. The lagging side's own rounds already pull. Documented in `docs/limitations.md`. |
| F07 backup falls back to a replacing rename | fixed | `c73cde7` | No fallback: any hard-link error except `AlreadyExists` fails the backup. There is no Linux test that can force hard links to be unsupported; the change only removes code. |
| F08 staging name too long for long destinations | fixed | `c73cde7` | Staging is `.ma2a-backup-<32 hex>/backup.sqlite`. The test uses a 247-byte destination name. |
| F09 pre-existing download directory accepted | fixed | `b1c8d13` | On Unix it is used only if `uid == geteuid` and `mode & 0o077 == 0`; otherwise `AlreadyExists`, left untouched. |
| F10 `report`/`REPORT` share progress | fixed | `b1c8d13` | The partial file hashes the exact name. The lock (`*.ma2a-lock`) hashes the lowercased name, so a case-insensitive volume still has one writer. On a case-sensitive volume, two destinations differing only in case wait for each other, which is safe. |
| F11 destination named `.ma2a-downloads` | fixed | `b1c8d13` | Refused with `InvalidInput`, in any case, before anything is created. |
| F12 Files can address the download directory | fixed | `b1c8d13` | `.ma2a-downloads` is in the Files reserved prefixes, in every letter case. A root whose path passes through such a directory answers `invalid_path` for every operation. |
| F13 two-member Space cannot recover once all addresses expire | deferred | — | It is a feature: an explicit import of a fresh signed address. It stays documented in `docs/limitations.md`. |
| F14 Windows ACL predicate accepts partial rights | fixed | `5bc4fa8` | Requires exactly one rule each for the user and SYSTEM, both with every FullControl bit. The Windows-only test runs in the CI `native` job on `windows-latest` (it cannot run here). |
| F15 upload sweep loses a staging file after a parent rename | deferred | — | It needs a Runtime-controlled same-volume staging directory, which is a redesign. The leaked file is only disk use under a name callers cannot address. Documented in `docs/limitations.md`. |
| F16 unbound Agent Process cleanup gives up | deferred | — | Persisting the pending ProcessId needs a schema change and restart recovery. Releasing on a bind error was considered and rejected: it would leave the still-current instance without a Process and without a successor. Documented in `docs/limitations.md`. |
| F17 Windows custom A2A token inherits the parent ACL | fixed | `243e563` | `ma2a_store::protect_private_file` (new public wrapper of `protect_new_file`) runs on the staged token before it is written. On failure the staged file is removed. The Windows path is checked by the CI `native` job. |
| F18 a token file can take another adapter's lock name | fixed | `243e563` | Locks are `STATE_DIR/a2a-locks/<32 hex of BLAKE3(canonical parent + name)>.lock`, lowercased on Windows. A token file inside `a2a-locks/` is refused. |
| F19 oversized exposure list gives no way to find entries | deferred | — | The exact-version local API makes paging a protocol change, and the state needs an unreleased build. Documented in `docs/limitations.md`. |
| F20 E2E cleanup deletes near-matching names | fixed | `d1da8a5` | Names must match exactly: fixed names, or `run-N.SUFFIX` with N all digits. Only the entries listed by the check are removed. `scripts/test-e2e-output-reuse.sh` (a stand-in failing `cargo` stops the script right after the check) runs in the E2E workflow. |
| F21 SBOM lists every platform's crates | fixed | `d6be7a1` | `generate-sbom.sh ARCHIVE REPORT OUT TARGET` runs `cargo metadata --filter-platform TARGET` and keeps the closure of `ma2a-app` over normal dependency edges. Locally that is 349 Rust packages for Linux and 360 for Windows (previously 482 for both). Validation refuses `xtask`, and a failure probe covers it. |
| F22 interrupted unsorted test list blocks reuse | fixed | `d1da8a5` | `run-N.tests.txt.unsorted` counts as the script's own. |
| F23 receipt recorded for a non-removal | fixed | `7a16f03` | Persistence refuses `answering()` unless the sender was a member before, is not one after, and is revoked. Nothing is committed. |
| F24 backup publishes whatever the staging name holds | fixed | `c73cde7` | The staging directory is fresh and owner-only, and the file handle stays open. On Unix both the staging name (before the link) and the destination (after) must have the held file's `(dev, ino)`. On a mismatch after the link, the backup fails but does not remove what is there, because it may not be ours. |

## Found along the way

- **`871b68f` Windows PowerShell module path.** The new Windows CI test for F14 failed at the
  very first step, applying the ACL. `powershell.exe` started from a PowerShell 7 session (GitHub's
  Windows shell, or any user's `pwsh`) inherits `PSModulePath`, cannot load
  `Microsoft.PowerShell.Security`, and so has no `Set-Acl`. The Store then refused to open its
  state directory. The child now starts without `PSModulePath`. The Windows CI job also runs
  `tests/key_permissions.rs`, which exercises real key-file creation.
- **`2b4f7f0`, `c595a28` agent test races.** Under full load,
  `adversarial::unknown_stopped_foreign_and_cross_space_agents_are_refused` used a grant before
  it had replicated to the Agent host, and it outlived the 30 s task it expected to be
  `running`. It now retries while the host answers `unauthorized`, and accepts `running` or
  `completed` (that is, not cancelled).
- **`87875a6` lifecycle saturation test.** It failed twice in CI's `quality` job: a stalled
  call whose handshake missed the 2 s I/O deadline gave its slot back, so the pool never
  filled. Stalled calls that end are now replaced until every slot is held.

The open questions ("truncated push with a relay advertisement", "control-response permit reclaim
under back-pressure") are still unconfirmed and were not changed.

## Lessons

- **A refusal must happen before signing.** `SpaceManifestV1::new` and `SpaceChain::apply` both
  reject an over-limit manifest, but as `ProtocolError`/`ManifestError`. The actor treats those as
  fatal. Any limit a remote request can hit needs its own Store error listed in
  `is_request_refusal`.
- **One key does not fit both jobs.** For download helpers, exact-name keys keep progress apart,
  and case-folded keys keep a single writer. Using one key for both always breaks one of the two
  file-system kinds.
- **Locks need their own namespace.** Any lock file named after a user-chosen path can be chosen
  as another instance's data path.
- **Do not clean up what you cannot prove is yours.** After a failed identity check on a
  published backup, report the failure instead of removing the entry.
- **The Windows cross-check does not run here.** `cargo check --target aarch64-pc-windows-msvc`
  fails in blake3's C build script. Windows-only code is compiled by the CI `native` job on
  `windows-latest`, which now also runs the `windows_acl` Store test.

## Verification

- `cargo build --workspace --bins && cargo run --locked -p xtask -- check`: exit 0, 979 tests
  passed (nextest), plus web checks and 151 web tests, cargo-deny, pins, and LOC policy clean.
- `MA2A_E2E_RUNS=1 ./scripts/run-e2e.sh`: 72 tests, 13 evidence records.
- `./scripts/test-e2e-output-reuse.sh`: `run-e2e output reuse: ok`.
- SBOM: `generate-sbom.sh` for `x86_64-unknown-linux-gnu` and `x86_64-pc-windows-msvc`, then
  `validate-release-metadata.sh` passes for both. An SBOM with `xtask` added is rejected.
- Windows: the CI `native (windows-latest)` job compiled every change in this round. Its
  native ACL test run at `5bc4fa8` is what exposed the `PSModulePath` defect.
