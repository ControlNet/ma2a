# MA2A Round 9 Review & Fix Summary

Baseline: `master` / `origin/master` at `19a42fb`.

## 1. Round-9 Findings Fixed

### Blocking (B01–B03)
- **B01 (`RUSTSEC-2026-0592`)**: Updated `Cargo.lock` (`url 2.5.8 -> 2.5.9`, `form_urlencoded 1.2.2 -> 1.2.3`) via `cargo update -p url --precise 2.5.9`, restoring `cargo deny check` and `cargo run --locked -p xtask -- check` on Rust 1.91.0.
- **B02 (Windows cold start, `space invite` ACL & leak, `smoke-release.sh`, test fixtures)**:
  - `crates/ma2a-runtime/src/ipc/windows.rs`: replaced plain `fs::create_dir_all` with `ensure_private_dir` (validating local absolute non-UNC paths, using `create_protected_dir` + `protect_private_directory` / `ensure_private_directory`).
  - `crates/ma2a-services/src/windows_files.rs`: added `create_protected_file` (Win32 `CreateFileW` with protected SDDL `D:P(A;;FA;;;{user})(A;;FA;;;SY)`) and exposed `rename_no_clobber` (with interior-NUL rejection in `files/rename_windows.rs`).
  - `crates/ma2a-runtime/src/ipc/server/execute/{space.rs,invite_file.rs}`: moved invitation file writing onto `spawn_blocking` and staged Windows invitation tickets inside a protected staging directory with `create_protected_file` + `rename_no_clobber` (zero `powershell.exe` spawns, safe on `\\?\` paths, guaranteed cleanup on failure).
  - `crates/ma2a-app/src/commands/workflows/space_invite.rs`: removes temporary invitation file even when `read_owner_only_invitation` fails.
  - `crates/ma2a-store/src/permissions.rs`: enforced `validate_local_absolute` in `protect_private_directory` and normalized `\\?\C:\...` verbatim disk prefixes before PowerShell `Set-Acl`/`Get-Acl`.
  - Updated Windows test fixtures (`daemon_fixture.rs`, `client_deadline_tests.rs`, `ipc_lifecycle.rs`, `server_tests/support.rs`) and `scripts/smoke-release.sh` (skip `mkdir -m 700` on Windows, quote `"$binary"`, strip `\r`, pass `curl` bodies via `--data-binary @- < "$file"`).
- **B03 (Windows flat `.zip` release validation)**:
  - `scripts/validate-release.sh`: accepts flat-root `.zip` on `*-pc-windows-msvc` while rejecting mixed/stray-dir/incomplete archives; replaced regex `sed` prefix stripping with literal `awk` prefix matching and CRLF stripping.
  - `scripts/test-release-failures.sh`: added Windows flat/mixed/stray-dir/incomplete `.zip` and dot-prefix `.tar.xz` probes plus `shasum -a 256` fallback.

### High (H01–H09)
- **H01 (`Partial` sidecar & leak contract)**:
  - Added `Partial::flush`, `Partial::has_bytes` (flushing in-flight async writes first), and `Partial::finish_or_discard` for non-resumable callers.
  - `agent_artifact.rs`, `remote/agent.rs`, `agents/tools/files.rs`: fetch first chunk / acceptance before `Partial::open`, call `partial.begin(identity)`, bound chunk offsets via `checked_add`, and publish with `finish_or_discard`.
  - `remote/transfer.rs`: discards 0-byte partials cleanly, flushes preserved partials in `abandon`, and checks `partial.identity().is_some()` on `PreconditionFailed`.
- **H02 (`dir_windows.rs`)**: `Directory::open_root` verifies `is_dir()` and stores `windows_files::final_path(&handle)?`.
- **H03 (Windows ACL/SDDL/UAC parity)**:
  - `sddl.rs`: accepts SDDL aliases `LA`, `LS`, `NS` for matching current user SIDs.
  - `windows_files.rs`: replaced `OW` with explicit `current_process_user_sid()?.to_sddl()?` + `SY`, switched `FileId` to 128-bit `GetFileInformationByHandleEx(FileIdInfo)` (safe on `ReFS`), guarded null SDDL pointer, and bounded `final_path` growth.
  - `relay_tls_windows*.rs` & `space_windows*.rs`: aligned owner checks across `{CurrentUser, SYSTEM, BUILTIN\Administrators}` and handled `LocalSystem` (`user == system`) ACE matching cleanly.
- **H04 (`control_sync` truncated push)**: `control_sync/pages.rs` returns early when `omitted` after manifest prefix (matching `pull_page`), filters pushed relay advertisements with `allows_private_relay_provider`, and adds a regression test in `staging/tests.rs`.
- **H05 (Consumed enrollment replay)**: `ma2a-store/src/enrollment.rs` checks invitation expiry and current Space membership before returning `EnrollmentOutcome::Retry`, with regression test in `space_members_full.rs`.
- **H06 (Inbound revocation race & `AgentRequest::Artifact`)**:
  - `FrameWriter` tracks `terminal_sent()`; `stream_host/inbound.rs` checks `stop.is_cancelled()` right after `watch.bind`, uses `biased;` `select!`, and resets QUIC streams only when `writer.is_torn() || (stop.is_cancelled() && !writer.terminal_sent())`.
  - Wired `stop` into `agents/artifacts.rs`, `processes/reads.rs`, `agents/watch.rs`, `exec.rs`, `shell.rs`, and `proxy.rs`; bounded terminal writes in `processes/host.rs` and `agents/host.rs` with `FINAL_WRITE_GRACE = 5s`.
- **H07 (`WebAuthService`)**: Removed per-request `Repository::open` in `web/auth.rs`, reusing persistent `sessions: Arc<Mutex<Repository>>` while keeping Argon2id work outside the mutex.
- **H08 (Web/IPC timeouts, locks, listener resilience)**:
  - `ipc/server/dispatch.rs`: acquires `context.replay` only when `command.request_id().is_some()`.
  - `web/router.rs` & `web/runtime_mutations.rs`: raised default timeout to 120s, removed `/api/v1/spaces/redeem`, and expired cookies on `session_revoke_all` only on non-error payloads.
  - `web/lifecycle.rs`: `.http1_only()` (preventing untimed `auto::Builder` `ReadVersion` / `h2c` connection-cap exhaustion), 64-connection semaphore, 10s `header_read_timeout`, transient `accept()` backoff, and `JoinSet` reaping.
  - `web/src/api/{mutations-schema.ts,mutations.ts,client.ts,services.ts}`: raised timeouts (120s mutations, 30s reads), parsed nullable `remediation` into `RuntimeMutationRejectedError`, and separated per-form status/busy guards in `spaces.tsx`, `relay-config.tsx`, and `settings.tsx`.
- **H09 (`unsafe_code` confinement & cross-target Clippy)**:
  - Added `#![forbid(unsafe_code)]` / `#![cfg_attr(not(windows), forbid(unsafe_code))]` on `ma2a-agent`, `ma2a-service`, `ma2a-runtime`, `ma2a-app`.
  - Replaced substring scan with Rust lexical scanner in `xtask/src/unsafe_scan.rs` (handling char/byte/Unicode escapes, raw identifiers `r#unsafe`, nested block comments, attribute bracket scope, whitespace between lint name and `(`, and `warn(unsafe_code)`), removed self-exclusion in `workspace_tests.rs`, added `ci.overrides` check, enabled `clippy` across the `native` CI matrix, and cleared all Windows Clippy errors across all workspace libs, bins, and tests.

### Medium & Low (M01–M07, L01–L03)
- **M01/M02**: `files/ops.rs` rejects empty/control-char filenames and tracks `last_scanned` cursor; `anchor_unix.rs` / `anchor_windows.rs` map `NotFound`/`NotADirectory` in `is_at` (with full share flags on Windows); `transfer_staging.rs` uses `INSERT OR REPLACE` while `upload.rs` preserves journal on `AlreadyExists`; `agents/tools/{workspace,files,agent}.rs` reject `:`, trailing `.`/` `, `.ma2a-downloads`, Windows reserved device names, and re-validate `destination` after stream acceptance.
- **M03/M04**: `address_lookup.rs` caps cross-Space addresses at `MAX_ADDRESS_RECORD_ADDRESSES`; `processes/{reads,stdin,host}.rs`, `agents/{host,watch}.rs`, `supervisor/host_child.rs`, `ma2a-service/src/host.rs`, and `ma2a-agent/src/sdk/task.rs` close flow-control, lease-release, grace-shortening, notice-close, and oversized-prompt hangs.
- **M05/M06**: `web.rs` parses multi-header `Cookie` while rejecting duplicate `ma2a_session`; `app.tsx` unwraps `WebAuthenticationError.cause` for 401/403 logout cleanup; `runtime-controller.ts` fixes retry continuity, `recoveryPending` ownership, and `detailWorkers` draining across `clearDetails()`; `secret_argv.rs` strips `--state-dir` before `--` and anchors subcommands at index 0 (preserving `exec --` / `process start --` passthroughs); `output.rs` counts private+public relay candidates; `a2a.rs` prints `claim.path()`.
- **M07/L01–L03**: Fixed CSS `url(...)` quoted/unquoted regex in `extract-runtime-references.pl`; hardened SPDX 2.3 SBOM (`created` UTC `Z`, `documentDescribes`, `crates.io` URLs, `/` -> ` OR ` licenses) and Windows CRLF handling in `release.yml`, `generate-sbom.sh`, `validate-release-metadata.sh`, and `run-e2e.sh`; fixed `xtask/src/{fs,loc}.rs`; updated `build.rs`, `relays.tsx`/`coverage-matrix.tsx` React keys, and protocol/CLI/quickstart docs.

## 2. Verification Commands

```bash
cargo run --locked -p xtask -- check
CFLAGS_x86_64_pc_windows_gnu=-D_WIN32_WINNT=0x0601 \
  cargo clippy --locked --target x86_64-pc-windows-gnu --workspace --all-targets -- -D warnings
bash -n scripts/*.sh && sh -n scripts/run-e2e.sh scripts/check.sh
./scripts/test-runtime-references.sh && ./scripts/test-e2e-output-reuse.sh
```
