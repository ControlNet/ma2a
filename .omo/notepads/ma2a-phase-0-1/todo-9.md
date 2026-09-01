# Todo 9 Enrollment Findings

- Implemented short-lived owner-authorized, single-use enrollment over authenticated Iroh.
- Invitation plaintext secrets remain only in encoded tickets; SQLite stores a domain-separated digest.
- Owner redemption atomically commits invitation consumption, authenticated candidate identity, RequestId, generation N+1, canonical response chain, and Runtime revision.
- Exact same-candidate/same-RequestId retries survive restart and return the original committed chain; wrong-candidate or changed-request replay fails with conflict.
- Candidate Runtime membership changes only after complete canonical chain validation and durable persistence.
- Preserved the existing generic invitation redemption contract by allowing consumed rows where both enrollment replay fields are absent.
- The reserved enrollment ALPN changes the zero-Space test contract: a successfully negotiated enrollment connection must be explicitly closed before Runtime shutdown.
- Full verification: `cargo nextest run --workspace --all-features` passed 121/121; strict Clippy, rustfmt, and Secret Guard tracked scan passed.
- LSP diagnostics were unavailable because the isolated `/tmp/opencode/ma2a-todo-9` worktree is outside the request cwd accepted by the diagnostics tool.

## 2026-08-29 independent review remediation

- Invitation lifetime is now a typed positive interval capped at 300,000 milliseconds at creation and decode boundaries.
- Candidate requests no longer carry authoritative redemption time; the owner Runtime injects `RuntimeClock`, and Store redemption receives an owner-authorized typed value.
- Enrollment responses now stream bounded canonical `EnrollmentPage` frames with per-operation ten-second deadlines instead of one aggregate response allocation.
- Core v1's 255-manifest, 64-member, 64-revocation, and 64-byte-label bounds keep every valid chain below 4 MiB; the maximum 255-generation chain round-trips as 32 pages, while the live generation-58 test proves multi-page Iroh delivery.
- Denied expiry, cancellation, replay conflict, and cross-Space redemption preserve repository revision and chain generation.
- Store no longer depends on the network crate; candidate startup restores durable memberships from verified Core chains.
- Runtime persists and reuses its Iroh UDP bind port, so exact owner-restart retries remain reachable through the signed ticket address.
- Full remediation verification passed: locked workspace check, strict Clippy, rustfmt, and 124/124 workspace nextest tests. LSP remained unavailable for the isolated `/tmp` worktree.
- Remediation landed as 12 English Conventional Commits from `5710f18` through `0404d7e`; each staged payload and the final 204-file tracked tree passed Secret Guard.

## 2026-08-29 second independent review remediation

- Enrollment creation now accepts only a bounded lifetime; the owner Runtime clock supplies the signed creation time and checked expiry.
- Invitation rows persist and compare creator Endpoint ID, owner-created time, expiry, and bounded canonical owner bootstrap bytes alongside only the domain-separated secret digest.
- Candidate Space identity and local membership are checked before durable persistence; owner redemption preserves outstanding revocations and rejects already-current members without a redundant generation.
- Ticket secret copies zeroize on drop, ticket body temporaries use zeroizing storage, and ticket encoding no longer silently substitutes an empty body.
- Enrollment framing now enforces 32 total pages before allocation, a 30-second aggregate exchange deadline, bounded mailbox admission, and bounded peer-close coordination.
- Runtime display names use Core's 64-byte member-label bound, and cancellation no longer accepts an ignored timestamp.
- Focused red evidence reproduced caller-selected timestamps and acceptance of page count 33 before the shared-boundary fixes.

## 2026-08-29T21:52:54+10:00 third independent review remediation

- Red live-Iroh regressions proved that EOF-based framing accepted success count zero, success count 33, denial count one, and an extra frame after the declared count; the actual server emitted no explicit count (`[7]` for denial and `[0, 0, 0, 0, 4, ...]` for one success page).
- Enrollment responses now use exactly `status:u8 || page_count:u16 big-endian || page_count * (page_len:u32 big-endian || page_bytes)`. Server status/count and all page lengths are preflight-validated before writes; the client validates count before capacity, bounds each length before allocation, reads exactly the declaration, and rejects trailing bytes.
- A deterministic Runtime/SQLite regression moved the owner clock from ticket creation T=201 to T-1=200. Before the fix it established generation 1; after the Store lower-bound check it returned the existing `EXPIRED` denial and preserved revision, generation, invitation pending status, and candidate membership.
- Invite decoding now borrows the secret and signature slices until signature-length and trailing-byte structural checks finish, then materializes the protected ticket arrays. Malformed trailing-ticket coverage remains green.
- Focused verification passed six net framing tests, the not-yet-valid app scenario, and the malformed-ticket test. Relevant package tests passed 78/78, all seven enrollment app scenarios passed, and full workspace nextest passed 135/135 with rustfmt, locked check, strict Clippy, and `cargo xtask check-loc` successful.
- LSP diagnostics were attempted for every changed Rust file and uniformly returned `LSP file path must be inside request cwd: /tmp/opencode/ma2a-todo-9/...`; Cargo, Clippy, rustfmt, LOC, live Iroh, Runtime/SQLite, and nextest are the fallback evidence.
- Commits: `7be782801452d3f2c2eff0a3b092602f993cccb1`, `369f55ac4cbab4208443974a3c069c0cfe88838e`, `141efbd77f37a87ba78c1c156954b9e5f365b5a9`, `120a9166811db4b10ec962a030b7c289524b1ed3`.
- Every staged payload and the final 210-file tracked tree passed Secret Guard. Final product status contains only protected untracked `.omo/`; no push or history rewrite was performed.

## 2026-08-29 final Rust policy correction

- Reproduced the remaining policy failure exactly: the programming-skill no-excuse checker reported
  `[box-dyn-error]` at `crates/ma2a-net/src/enrollment/tests.rs:14`.
- Replaced the boxed dynamic test error alias with `std::io::Error` and explicit error conversion at
  Iroh, Tokio timeout/task, Runtime endpoint, stream, router, and shutdown boundaries. The six live
  Iroh tests and their response-byte assertions are unchanged.
- Commit `c7e3a99` (`test(net): type enrollment harness errors`) contains only the test-harness
  correction. Focused tests passed 6/6; full workspace nextest passed 135/135; rustfmt, locked
  workspace check, strict Clippy, `cargo xtask check-loc`, no-excuse policy, and staged Secret Guard
  passed.
