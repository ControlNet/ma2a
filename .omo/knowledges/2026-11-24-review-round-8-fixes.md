# Round-8 review: what was fixed, and the boundary each fix rests on

This closes out `2026-11-24-review-round-8.md`, the Codex report on baseline `4af6c24`. Each
finding was checked against the code before it was fixed. Fixes landed one commit per finding or
per tightly coupled group, each with a test that fails without it where one can run on Linux, and
a native Windows test where the behaviour is Windows-only. They run from `09d7bc8` to `6dbfee9`
on `master`.

## Status

| Finding | Status | Commit | Notes |
| --- | --- | --- | --- |
| F01 backup checks one directory, SQLite opens another | fixed | `09d7bc8` | `trusted_destination` returns the destination below the canonical directory it checked; staging, the SQLite open, and publishing all use it. Nobody else may rename a directory on that path, and it has no link left to repoint. |
| F02 Windows publish re-resolves a mutable alias | fixed | `0f4c803` | Each pinned directory is reached by `GetFinalPathNameByHandleW` of its own handle. Files are compared by volume serial and file index (`GetFileInformationByHandle`), so `end.rs` and `helpers.rs` run the same `is_at`/`names` checks on both platforms. Native test: a junction repointed mid-download publishes nothing and keeps both downloads' progress. |
| F03 Windows helper files writable by other accounts | fixed | `0f4c803` | The private directory is created by `CreateDirectoryW` with `D:P(A;OICI;FA;;;OW)(A;OICI;FA;;;SY)`; an existing one is used only when its owner and protected DACL (read by handle, parsed from SDDL) admit no other account. Helper files other than lock files share only reading. |
| F04 re-check confirms a revision before the root is bound | fixed | `af4390a` | A pending operation that passes leaves the revision undecided, so the next tick compares the root actually bound. |
| F05 Agent artifact downloads publish by path | fixed | `58c4652` | Local and remote `agent artifact --output` go through `client::resume::Partial` (`finish(false)`, `discard` on error); an existing output is refused up front and at publish. |
| F06 cleanup deletes an operator's `created` file | fixed | `d7158d0` | The marker is created exclusively with the content `ma2a-download-directory v1\n` and recognized only by it; `locks` is removed only in a marked directory or when this download made it. |
| F07 departure receipts grow without bound | fixed | `b1ab6fc` | At most `MAX_BOUND_DEPARTURES` (8) requests per sender and removal generation; beyond it `StoreError::DepartureReceiptsFull`, a request refusal that leaves the Runtime running. `cut_at` clones only the prefix at the target. |
| F08 A2A lock and token in different directories | fixed | `d8bd9c9` | `claim` resolves the directory once and returns it; writing and removal use `Claim::path()`. On Windows the `\\?\` prefix is dropped only when the plain spelling canonicalizes to the same path. |
| F09 Windows listing page undecodable | fixed | `87728d9` | Names over 255 UTF-8 bytes are skipped before paging, like non-UTF-8 names. Native Windows test with a 100-character CJK name. |
| F10 upload sweep ownership claim | fixed (docs) | `bed46b9` | `limitations.md` now says the sweep removes by name and tells operators to move `.ma2a-upload-*.part` entries away first. Recording an identity would need a journal schema change. |
| F11 parallel E2E runs swap JUnit | fixed | `6b0b5cb` | JUnit path from `cargo metadata`'s `target_directory`; removing, running, and copying happen under an `mkdir` lock beside it. |
| F12 oversized run count clears evidence first | fixed | `6b0b5cb` | Counts are 1–9999 (`?????*` refused) before the output directory is touched. |
| F13 SBOM bound by name only | fixed | `6dbfee9` | Validation recomputes the archive's SHA-256 and compares it with `documentNamespace`; a probe swaps in a same-named rebuilt archive. |
| F14 Agent tool staging names overflow | fixed | `58c4652` | The tools download through `Partial`, whose helper names are fixed-length hashes; a 255-byte destination is tested. |
| F15 failed ready line leaves the token | fixed | `d8bd9c9` | A `Published` guard owns removal from the moment the token is written. |

## Design decisions

- **Windows identity needs FFI.** `MetadataExt::volume_serial_number`/`file_index` are still
  unstable, so `crates/ma2a-services/src/windows_files.rs` (an audited unsafe boundary) provides
  `file_id`, `final_path`, `owner_and_dacl`, and `create_protected_dir`. The SDDL creation moved
  there from the A2A token code, which now calls it; the allowlist entry moved with it.
- **SDDL is parsed in safe code.** `sddl::is_private` accepts owner ∈ {user, `SY`, `BA`}, a
  protected (`P`) DACL, Allow entries only for those or `OW`, Deny entries, and nothing else. It
  is pure, so Linux unit tests cover it.
- **Pinned ancestors.** Windows refuses to rename a directory while a handle is open beneath it,
  so the final path of a pinned directory stays valid for the download; the publish checks still
  catch anything that slips past.
- **Old Windows `.ma2a-downloads` directories are now refused**: earlier builds made them with an
  inherited ACL. The docs say to remove one once no download uses it.

## Found along the way

- **The first native run of the round-7 Windows code passed** (`6fc0674`): Store ACLs, key files,
  the A2A token staging, Files tree removal, and the download tests.
- **`ma2a-net` `connection_paths` flakes in CI** with `Address already in use` from iroh's socket
  bind (run on `af4390a`, quality job). It is unrelated to these changes; watch whether it recurs.
- **The sidecar test fixture made its private directory plainly**, which Windows now refuses
  (inherited ACL); it uses `create_protected_dir` there, as the directory tests do.
- **rustfmt wraps a long `allow(unsafe_code, reason = …)`**, which hides it from the allowlist
  scan; keep the reason short enough for one line.

## Verification

- `cargo build --workspace --bins && cargo run --locked -p xtask -- check`.
- `MA2A_E2E_RUNS=1 ./scripts/run-e2e.sh`, `./scripts/test-e2e-output-reuse.sh`.
- `CFLAGS_x86_64_pc_windows_gnu=-D_WIN32_WINNT=0x0601 cargo clippy --target x86_64-pc-windows-gnu -p ma2a-services --all-targets -- -D warnings`
  (clean), and `cargo check` for the workspace on that target.
- SBOM: `generate-sbom.sh` on an archive, `validate-release-metadata.sh` passes; the same SBOM
  against a same-named archive with other bytes fails.
- CI `native (windows-latest)`: the new `windows_files`/`sddl` step, `files_host`, and the
  download tests, including the junction test.
