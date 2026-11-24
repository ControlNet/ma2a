# MA2A Round 10 Review & Fix Summary

Baseline: Codex reviewed `c4f72a2`; the report is `2026-11-24-review-round-10.md` (F01–F17).
Each finding was fixed in its own commit with a regression test where one could be written.

## Status

| Finding | Commit | Fix |
|---|---|---|
| F01 smoke reads JS references wrong | `a18c021` | `scripts/` tokenizes the bundle (strings, comments, regex, templates) before looking for runtime references. |
| F02 upload staging collides / sweeps | `4f263d8` | Each upload stages under a fresh random name it alone owns; the sweep never touches another upload's file. |
| F06 download length | `8126a21` | `download_content` takes the accepted identity and offset, refuses a chunk past the accepted size before writing it, and requires the terminal to match. |
| F12 partial check errors | `7b4934b` | `has_bytes` returns I/O errors; `file get` removes a partial only when its content is invalid or known empty, otherwise keeps it and reports the local failure. |
| F15 list names a request cannot carry | `2e6abb4` | Files list skips names with `:` or `\` (and others a later request would refuse). |
| F14 SBOM `documentDescribes` | `4d452a1` | The release check resolves the described package instead of trusting the first one. |
| F13 zip extra directories | `e011159` | Archive check refuses extra directories under the top directory. |
| F16 unsafe scan Unicode | `c2c4a58` | Identifiers are read as whole Unicode identifiers. |
| F17 LOC block comment | `912bdc3` | A block comment opened after code on the same line counts as a comment. |
| F07 web logout | `2fe2089` | Logout revokes the browser's current session, not the stale page's. |
| F10 SID aliases (LA etc.) | `bfc1836` | `sddl::is_private` takes an `is_user` matcher; on Windows `ConvertStringSidToSidW` resolves the alias, no guessing from RIDs. |
| F09 LocalSystem ACL | `c30bd79` | Owner-only checks accept one or two ACEs covering the user and SYSTEM (one ACE when user == SYSTEM); PowerShell apply dedups SIDs. |
| F11 Windows invite file | `a424b93` | `check_destination` before creating the invite; on Windows the output directory is pinned and staging/rename use its final path. |
| F03 Windows cold-start ACL | `50bf871` | `ensure_private_dir` creates every missing ancestor private, top down. |
| F05 Files recursion after revoke | `0270965` | A `Stop` flag set when the stream's blocking work is dropped; `remove_tree` checks it before each entry. |
| F08 stream first/last frames | `6d6f298` | Accept/reject/outcome frames are bounded by a 5 s grace and shutdown; attach bounds the supervisor's first answer to 10 s. |
| F04 workspace tool paths | `b80633a` | Reads open the file, re-resolve, compare identity, refuse links into `.ma2a-downloads`, and pass the open file on; downloads re-check the destination once its directory is held; shared Windows device-name rule with superscript digits. |

`900737d` moves the SDDL readers (`owner_and_dacl`, `current_user_matcher`) into the audited
`windows_acl.rs`, because F10 and F11 pushed `windows_files.rs` past the 250-line limit. Only the
full `xtask check` catches LOC overruns; run it before pushing, not just clippy.

## Decisions

- F04 is a path policy, not a security boundary: the Agent runs as the same OS user. A swap after the
  held-directory check is still possible; `Partial::finish` re-checks the parent at publish time.
  Documented in `docs/protocol/agent-v1.md` (Workspace paths).
- F09: the SYSTEM-only path (process running as LocalSystem) has no native CI test; it is covered by
  unit tests on the ACE/rule shapes only.
- F10: alias resolution is delegated to Windows; Linux unit tests use a closure that plays the matcher.

## Verification

- `cargo build --workspace --bins && cargo run --locked -p xtask -- check`
- `CFLAGS_x86_64_pc_windows_gnu=-D_WIN32_WINNT=0x0601 cargo clippy --locked --target x86_64-pc-windows-gnu --workspace --all-targets --all-features -- -D warnings`
- `MA2A_E2E_RUNS=1 ./scripts/run-e2e.sh`
- CI `ci` and `e2e` workflows on push (native Windows runs the selected tests listed in `ci.yml`).
