# Round-7 review: what was fixed, and the boundary each fix rests on

This closes out `2026-11-23-review-round-7.md`, the Codex report on baseline `210ac51`. Each
finding was checked against the code before it was fixed. Fixes landed one commit per finding or
per tightly coupled group, each with a test that fails without it where one can run on Linux.
They run from `80eccd2` to `6fc0674` on `master`.

## Status

| Finding | Status | Commit | Notes |
| --- | --- | --- | --- |
| F01 answered departure can still remove a later membership | fixed | `a6a8e75` | A departure from a non-member is answered with the latest removal of its sender and now bound to it through `Repository::bind_departure_receipt`, which refuses a generation that did not remove and revoke the sender and leaves the revision alone. The test covers: owner removes, the late departure is answered, the member rejoins, the retry gets the same chain and the new membership stays. The remaining case, a request answered before schema 11 and first retried after a rejoin, is documented in `enrollment-v1.md`. |
| F02 another shard's response counted as success | fixed | `539e9de`, `786d99c` | Each dial carries the Space IDs it asked about. A responder refuses any cursor it does not share, so a valid response has exactly one page per requested Space; a missing, repeated, or foreign page is refused before anything is applied. A refused response fails that request only (the peer is not reported synchronized); a local failure still ends the round. |
| F03 push progress overstated | fixed (docs) | `c850cd7` | A push re-sends the same prefix from the first manifest; two successful pushes can leave the peer's generation unchanged. |
| F04 download verifies one file and publishes another | fixed | `228d026` | The destination's directory, the private directory, and the lock directory are opened once at `open` and every helper operation goes through them (`*at` on Unix, pinned without delete sharing on Windows). On Unix, publishing checks that the destination's path still names the held directory and that the partial name still refers to the hashed file, then renames from the held private directory into the held parent. The rotation test (a→a2, b→a) publishes nothing and keeps both downloads' files. |
| F05 recursive Files removal deletes download progress | fixed | `08b2392` | The walk skips `.ma2a-downloads` in any case, keeps going through siblings, and ends with `not_empty`. Windows replaces `remove_dir_all` with the same per-entry walk, pinning each directory and removing links without entering them. |
| F06 old mixed-case partials lose ownership | fixed (docs) | `228d026` | `services-v1.md` names the old helper files, says such a download starts over, that the files can be removed by hand, and that older clients must be stopped first. Lowercase-only names hash the same in both layouts and still resume. |
| F07 lock keys do not follow the volume's name rules | fixed | `228d026` (download), `1e8005a` (A2A) | Both locks now carry the target's own name in a lock directory beside it (`.ma2a-downloads/locks/NAME`, `.ma2a-a2a-locks/NAME`), so the file system decides which spellings are one file. The download test probes the volume's case behaviour and asserts one writer on a case-insensitive volume and independent downloads on a case-sensitive one. |
| F08 backup checks a handle SQLite did not write | fixed | `d84b540`, `bfe9227` | Precondition, checked on Unix: every directory from `/` to the destination's (after resolving links) belongs to this user or root and grants no group or other write unless sticky. Cleanup removes the staged file only while it is still the held file. |
| F09 Windows token file inherits before it is protected | fixed | `1e8005a`, `fcae832` | The token is written in a fresh `.ma2a-a2a-stage-HEX` directory. On Windows it is created by `CreateDirectoryW` with `D:P(A;OICI;FA;;;OW)(A;OICI;FA;;;SY)` already in place, then given the Store's exact ACL. A native test reads that ACL before any other step. |
| F10 different state directories lock one token separately | fixed | `1e8005a` | The lock moved beside the token file, so every Runtime of the account contends for it. |
| F11 token write removes an unclaimed staging file | fixed | `1e8005a` | Random staging names, `create_new` only, taken names are left alone; an injected-name test covers it. |
| F12 Windows Store accepts other owners | fixed | `80eccd2` | The validator requires owner = current user, SYSTEM, or Administrators. The ACL helper also received a null path before this commit (`-Command` swallowed `$args`), so Windows ACLs had never worked; the path now travels in `MA2A_ACL_PATH`. |
| F13 SDK handlers outlive `serve_on` | fixed | `bb69b69` | Handlers live in a `JoinSet` owned by the loop and are aborted, with the task table cleared, on every exit. |
| F14 restart said to end an unbound Process | fixed (docs) | `c850cd7` | Points to `ma2a process list`, `inspect`, and `terminate`. |
| F15 upload sweep can remove a restored entry | fixed (docs) | `c850cd7` | A same-named entry can only be a copy of that upload's own staging file (reserved prefix, random request id). |
| F16 oversized exposure list from direct Store writes | fixed (docs) | `c850cd7` | Recovery is `ma2a service unexpose` by name. |
| F17 SBOM lists proc-macros as shipped | fixed | `f252a5a` | The closure stops at proc-macro packages (Linux 349 → 309, Windows 360 → 319). Validation requires `serde` and refuses `serde_derive`; a failure probe covers it. |
| F18 `MA2A_E2E_RUNS=00` runs nothing | fixed | `cf2e287` | Positive integers without leading zeros only, checked before the output directory is touched. |
| F19 trailing slash refuses reuse | fixed | `cf2e287` | Trailing separators are stripped first. |
| F20 operator's empty download directory removed | fixed | `228d026` | A download that makes `.ma2a-downloads` writes a `created` marker; only a marked directory is ever removed. |
| F21 one Store failure skips revocation re-checks | fixed | `06f5c15` | A revision is marked seen only when every check was decided. |
| F22 Exec treats read/wait failures as clean ends | fixed | `4f16f8f` | A read error is a stream error; a wait error is `Termination::Unknown`. |

## Trust boundary these fixes rest on

- Processes running as the same account are out of scope: they can read the state itself.
- Backups: every directory on the destination's path must be owned by the user or root and not
  writable by group or others unless sticky. A group-writable home directory is refused too, even
  for the user's own group (on macOS that group is `staff`, shared by every user).
- Downloads and the A2A token reuse an existing private directory only when this account owns it
  and nobody else may enter it, and remove only directories a download made.

## Found along the way

- **Windows cross-check works locally after all.** `cargo check --target x86_64-pc-windows-gnu`
  builds with the system `x86_64-w64-mingw32-gcc` once SQLite sees a newer Windows API:
  `CFLAGS_x86_64_pc_windows_gnu=-D_WIN32_WINNT=0x0601 cargo check --target x86_64-pc-windows-gnu --workspace --all-targets`.
  The `aarch64-pc-windows-msvc` target still fails in blake3's C build.
- **Windows-only clippy debt.** `cargo clippy --target x86_64-pc-windows-gnu` fails in
  `ma2a-net/src/relay_tls_windows*.rs` (`borrow_as_ptr`, `needless_pass_by_value`,
  `missing_const_for_fn`) and warns in several Windows-only test files. CI never ran clippy on
  Windows. The Store and download code changed in this round is clean; the rest is untouched.
- **The Windows ACL helper never received its path** until `80eccd2`, so every Windows Store
  ACL apply or check before it failed.
- **macOS reports `st_dev` as `i32`.** `u64::from` does not build there; `8ac73cd` widens
  device and inode numbers from `fstat`/`statat` to `i128` on both sides of a comparison.
  The Linux gate cannot catch this; only the CI `native (macos-15-intel)` job does.
- **The Store test fixture never protected its state directory on Windows.** Once the ACL
  helper worked, `key_permissions` failed natively because the fixture's directory inherited
  the temporary directory's ACL; `6fc0674` applies `protect_private_directory` there.

## Lessons

- **Check by handle, not by name, when the content was checked by handle.** Anything verified
  through an open file must be published and cleaned up relative to directories held since the
  same moment; a path re-resolved later can name another directory with identical contents.
- **Let the file system decide name equality.** A lock that carries the target's own name in a
  sibling directory gets the volume's case and normalization rules for free; hashing a folded
  spelling always gets one kind of volume wrong.
- **Prove ownership before removing.** A directory may only be removed by whoever can show they
  made it; a marker file records that across processes.
- **A response must answer the request it was sent for.** Checking only that pages belong to
  shared Spaces lets one request's answer pass for another's.

## Verification

- `cargo build --workspace --bins && cargo run --locked -p xtask -- check`.
- `MA2A_E2E_RUNS=1 ./scripts/run-e2e.sh`.
- `./scripts/test-e2e-output-reuse.sh`.
- SBOM: `generate-sbom.sh` for both targets, then `validate-release-metadata.sh`.
- Windows: `CFLAGS_x86_64_pc_windows_gnu=-D_WIN32_WINNT=0x0601 cargo check --target x86_64-pc-windows-gnu --workspace --all-targets`,
  and the CI `native (windows-latest)` job, which now runs the Store ACL, key-file, A2A token,
  Files host, and download tests.
