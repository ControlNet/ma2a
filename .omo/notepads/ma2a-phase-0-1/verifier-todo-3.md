# Todo 3 Verification

## 2026-08-29 - Independent post-publication cleanup review at `d4ba0cc`

Scope was limited to `KeyStore::write` when `hard_link` succeeds and removal of the
same-directory temporary name fails. Product files were not changed.

### Evidence

| Step | Observable state after success | Observable state on failure |
|---|---|---|
| `OpenOptions::create_new` | A new temporary name exists; the destination is absent or independently pre-existing. Unix requests mode `0600`; Windows protection is applied next. | No publication has occurred. Cleanup is attempted and the write returns an I/O error. |
| `protect_new_file` | The temporary file has owner-only mode or the current-user-and-SYSTEM DACL. | The temporary name may remain with incomplete protection if best-effort cleanup also fails; the destination is untouched. |
| `write_all` | The complete secret is visible through the temporary name but is not yet proven durable. | The temporary file may contain a prefix; cleanup is attempted; the destination is untouched. |
| temporary `File::sync_all` | Secret data and file metadata have been flushed before publication. | Cleanup is attempted; the destination is untouched and no success is reported. |
| `hard_link(temporary, destination)` | This is the namespace commit/linearization point: destination and temporary are two names for the same protected file. Creation is no-replace; an existing destination is not overwritten. | `AlreadyExists` becomes `ProtectedKeyAlreadyExists`; other errors become `StoreError::Io`; the temporary is cleaned up best-effort. |
| `remove_file(temporary)` | Only the immutable destination name remains. | Both names remain, but the destination is already externally visible and readable. Current code returns `StoreError::Io` immediately and skips parent-directory sync and final validation. |
| `sync_parent(destination)` on Unix | The destination creation and temporary-name removal are made durable at the directory level. | Destination remains visible in the running system, but crash persistence of the directory entry is not guaranteed. Windows currently treats this step as a no-op. |
| `validate_private_file(destination)` | `write` returns `Ok(())`. Repository methods may validate and commit this opaque reference into SQLite. | The destination has already been published; the call returns the permission/metadata error. |

`Repository::set_endpoint` and `Repository::create_space` validate the published destination
before storing its opaque reference. They do not resolve an earlier ambiguous `KeyStore::write`
result automatically.

### Decisive edge analysis

- First call: after successful `hard_link`, a failed temporary unlink causes
  `Err(StoreError::Io(_))`, although `KeyStore::read` can already read the destination. The
  temporary name is an owner-only orphan pointing to the same file; it is not a partial or
  replacement publication.
- Same-reference retry: a fresh temporary file is written and synced, then `hard_link` sees the
  already-published destination and returns `ProtectedKeyAlreadyExists`. The API cannot distinguish
  its own prior committed write from a conflicting earlier writer, so the retry does not resolve
  the first call's false failure.
- This is therefore two separate facts: the leftover temporary name is a cleanup problem, while
  returning failure after destination publication is an ambiguous commit and retry-safety problem.
  On Unix it also skips the required parent-directory sync, so destination durability remains
  uncertain despite the secret file itself having been synced.
- Unix: Rust uses `linkat` and `unlink`. A successful link does not replace the destination; a
  failed unlink leaves both names. Open file descriptors normally do not prevent unlink, but
  permission, immutable/read-only filesystem, busy/NFS, or I/O failures remain valid outcomes.
  File `fsync` alone does not persist the containing directory entry.
- Windows: Rust uses `CreateHardLinkW`, `DeleteFileW` (with a readonly-file fallback), and
  `FlushFileBuffers`. Sharing denial from another process or antivirus can make deletion fail after
  hard-link creation. The protected DACL belongs to the linked file, so the destination remains
  protected; the extra name is still cleanup debris, not a failed publication.

### Smallest correct remediation

Keep `hard_link` as the atomic no-replace publication operation. Once it succeeds, treat temporary
unlink as best-effort cleanup, always continue to parent-directory sync and destination validation,
and return success when those publication/durability checks succeed. If orphan cleanup must be
observable, expose it separately as a non-fatal diagnostic or later garbage collection result;
do not encode it as failure of the already-committed write. This does not reintroduce the former
replace-on-race behavior because destination creation remains `hard_link`, not `rename`.

Required regression test: add a deterministic post-link unlink failpoint or narrow filesystem
adapter and a test named
`temporary_cleanup_failure_after_publication_is_non_fatal_and_retry_cannot_replace`. The failpoint
must make only the first temporary removal return `PermissionDenied` after a real successful hard
link, record that parent sync is still attempted, and assert that the first write returns `Ok(())`,
the destination contains the original bytes with owner-only protection, and a second write using
the same opaque reference returns `ProtectedKeyAlreadyExists` without changing those bytes. The
existing barrier test cannot exercise this state because all cleanup operations use the real
filesystem and normally succeed; sleeps or stress loops would not make the edge deterministic.

### Verification

| Command/tool | Result |
|---|---|
| `git status --short` in `/tmp/opencode/ma2a-todo-3` | No output before or after verification; candidate remained unchanged. |
| `git diff --check d4ba0cc` | No output; passed. |
| `cargo nextest run -p ma2a-store --test key_permissions --test transactions` | 10 tests passed, including immutable concurrent publication, owner-only permissions, forced-termination rollback, and transaction tests. |
| Rust LSP diagnostics for `key_store.rs`, `key_permissions.rs`, and `transactions.rs` | Each was rejected before rust-analyzer because `/tmp/opencode/ma2a-todo-3` is outside the request cwd, matching the known limitation. |

**VERDICT: REJECT**

Todo 3 retains an explicit correctness issue: post-publication temporary cleanup failure is
reported as write failure after an externally visible no-replace commit, a same-reference retry
cannot disambiguate that commit, and Unix directory durability is skipped.

## 2026-08-29 - Independent remediation re-verification

Scope was limited to the remediation above `d4ba0cc`. The candidate contains exactly the modified
`crates/ma2a-store/src/key_store.rs` and untracked private
`crates/ma2a-store/src/key_store/tests.rs`; this verifier changed no product file.

### Remediation evidence

| Requirement | Evidence |
|---|---|
| Private seam and stable API | `PublicationOperations`, `FilesystemPublication`, and `KeyStore::write_with_operations` have no visibility modifier and are confined to the private `key_store` module and its `#[cfg(test)]` child. Public `KeyStore::write`, `KeyMaterial`, `StoreError`, and all caller-facing signatures are unchanged. |
| Production uses real filesystem operations | `KeyStore::write` always selects `FilesystemPublication`, which delegates directly to `fs::hard_link`, `fs::remove_file`, the platform `sync_parent`, and `permissions::validate_private_file`. |
| Pre-publication failures remain fatal | `write_temporary` still uses `create_new`, owner-only protection, `write_all`, and `File::sync_all`, each with `?`. Its error branch performs best-effort temporary cleanup and returns the original error. The hard-link error branch also cleans up best-effort and returns either `ProtectedKeyAlreadyExists` or the I/O error. |
| Publication remains no-replace | The only destination-creation operation is a real hard link. The original concurrent-writer test again produced exactly one success and one `ProtectedKeyAlreadyExists`, with the destination bytes matching the winner. No rename or replacement path was introduced. |
| Only post-publication unlink is non-fatal | After a successful hard link, only `remove_temporary` has its result ignored. `sync_parent(&destination)?` and `validate_destination(&destination)` still execute and remain fatal durability/security checks. |
| Deterministic injected edge | `CleanupDeniedOperations::hard_link` first performs real `fs::hard_link` and sets `published` only after success. `remove_temporary` returns injected `PermissionDenied` only when that flag is true; before publication it delegates to real `fs::remove_file`. The state path is namespaced by process ID plus an atomic counter, with no sleep, stress loop, environment failpoint, or public mock. |
| Real durability and security checks in the regression | The test adapter calls the real `sync_parent` and real owner-only destination validator. Flags are set at those calls, and first-call success proves both completed after the injected unlink denial. |
| Observable contract | The regression observes first-call success, parent-sync reachability, destination validation, original destination bytes, owner-only protection, any retained alias's protection and original bytes, a public-path retry returning `ProtectedKeyAlreadyExists`, and unchanged original destination bytes after the retry. |
| `temporary_aliases.len() <= 1` assessment | This does not materially weaken the functional proof. The injected remover returns before any unlink after a successful real hard link, so control flow leaves one alias in the isolated directory; independently, first-call success, real sync/validation, retry conflict, and unchanged bytes prove the blocker. Requiring exactly one alias would strengthen cleanup bookkeeping but is not necessary for publication correctness or security. |

### Regression mutation proof

The candidate files initially hashed to:

- `key_store.rs`: `6c6c82bf434053dddd7c3bd1dfdd054f66c590284eacfc2a64eb1c780a1931f6`
- `key_store/tests.rs`: `6c7d05ebeca4f6afc589839171f46357cf5aaee6c86da3e617eedf89c076943d`

A disposable copy changed only the post-link line from ignored cleanup to
`operations.remove_temporary(&temporary)?;`. The focused regression then failed at
`key_store/tests.rs:100`, specifically `assertion failed: first_result.is_ok()`. Restoring the
disposable file from the untouched candidate reproduced both hashes and returned the regression to
green. The disposable directory was removed, and final candidate hashes remained identical.

### Verification

| Command/tool | Result |
|---|---|
| `GIT_MASTER=1 git diff --check d4ba0cc` before and after | Passed with no output. |
| `GIT_MASTER=1 git status --short --untracked-files=all` | Exactly `M crates/ma2a-store/src/key_store.rs` and `?? crates/ma2a-store/src/key_store/tests.rs`. |
| Focused cleanup regression on candidate | 1 passed; the disposable regression mutation produced the required red result and the restored copy passed. |
| Focused concurrent publication plus forced-termination rollback | 2 passed. |
| `cargo nextest run -p ma2a-store` | 14 passed, 0 skipped. |
| `cargo fmt --all -- --check` | Passed with no output. |
| `cargo clippy -p ma2a-store --all-targets --all-features -- -D warnings` | Passed. |
| `cargo run --locked -p xtask -- check` | Passed; aggregate Rust suite reported 48/48, dependency policy/machete passed, frontend checks passed. |
| Pure LOC | `key_store.rs` 220 and `key_store/tests.rs` 118; both are within the 250-line ceiling. The production file is in the 200-250 warning band but this remediation split its private regression into the child module rather than exceeding the ceiling. |
| Secret Guard path scan of `crates/ma2a-store/src` | 14 source files scanned; no secrets detected and no protected fixture bytes printed. |
| Rust LSP diagnostics for both touched files | Rejected before rust-analyzer because the isolated `/tmp/opencode/ma2a-todo-3` path is outside the request cwd, reproducing the known tool limitation. Compiler and strict Clippy evidence are clean. |

The known Windows MSVC cross-check limitation is missing host `lib.exe`, not an observed source
failure. The reviewed seam preserves the existing Windows real-operation path and DACL validation.

**VERDICT: APPROVE**

The remediation closes the prior blocker: real hard-link publication remains atomic and
no-replace; only post-publication temporary-name cleanup is best-effort; parent sync and destination
validation still execute; retries cannot replace committed bytes; and all pre-publication write,
protection, file-sync, and hard-link failures remain fatal with best-effort temporary cleanup.

## 2026-08-29 - Local commit

- Commit: `9ef1395ea273484e8cee0228460906d21229d77b` (`fix(store): preserve publication success after cleanup failure`)
- Scope: `crates/ma2a-store/src/key_store.rs` and `crates/ma2a-store/src/key_store/tests.rs`
