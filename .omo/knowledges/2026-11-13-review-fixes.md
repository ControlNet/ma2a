# Review fixes (2026-11-13): lessons that apply beyond each fix

Fixes for the review ledger `2026-11-13-review-complete-findings.md` (R1–R17, D1–D7) landed on
local `master` after `ce387ad`. This note keeps what is reusable.

## CLI commands that read stdin

- `tokio::io::stdin()` reads on a blocking thread that cannot be cancelled. If `main` returns
  while such a read is pending, dropping the runtime waits for it, so the CLI hangs until the user
  sends EOF. Every command that forwards stdin (`exec --stdin`, `shell`, `process attach --stdin`,
  `process stdin`, `proxy connect`, `service call --stdin`) must therefore end through
  `commands::remote::exit_reading_stdin` (or `unless_reading_stdin` for an early error) on **every**
  outcome, including errors. Only a successful path calling `std::process::exit` is the bug.
- Ending the CLI is not enough if the stream never ends. `process stdin` used to wait for the
  caller's `end` at every layer. The supervisor now answers `Written(closed = true)` as soon as
  the child ends, and the Runtime relays an early supervisor answer (`processes/stdin.rs`).
  The supervisor exits `LINGER` (5 s) after the child ends, so a supervisor test needs a `Watch`
  connection open to model the Runtime's follower.

## Control messages and bounded channels

- Never send a must-arrive control message (such as a hosted service's `stop`) with `try_send`
  into a bounded channel that also carries data. Stale call notices filled the buffer and dropped
  the stop. The link now selects on the service's `CancellationToken` with `biased` ahead of the
  buffer (`hosting/link.rs`, `LinkHandle::next`).

## Local file ownership

- An exclusive lease per destination (`client/resume/lease.rs`) uses `File::try_lock` (std,
  Rust ≥ 1.89) on `DEST.ma2a-part.lock`. On Unix the lock file is removed while still locked, and
  an acquirer checks `dev/ino` against the path afterwards to detect a removed file. On Windows
  it is opened without `FILE_SHARE_DELETE`, so it cannot be removed while open.
- An atomic no-clobber publish uses `hard_link(part, dest)` (fails with `AlreadyExists`) and then
  removes the part; check-then-rename is racy. A resumed partial file is opened in append mode, so
  restart by recreating it rather than with `set_len` on that handle.

## Local API

- The handshake precedes every command and every stream or hosted upgrade. Taking the replay
  mutex for it made one slow mutation delay everything, including hosted-service accepts with a
  10 s deadline. Only the handshake and the Web UI lifecycle skip the lock now. Tests that need a
  call stuck behind a held lock must use a real command such as `status`, not `probe()`.
- An operator mutation should answer with the revision from its own `Committed` receipt, not by
  reading the current revision afterwards (done for hosted services and Agent define/undefine;
  Agent submit/send/cancel still read it afterwards).
- `cargo run --locked -p xtask -- check` also runs `cargo deny check`, which can fail on newly
  published RustSec advisories even when `Cargo.lock` is unchanged; that is a dependency-update
  decision, separate from code fixes.
- Direct dependencies are exact `=x.y.z` pins in the workspace `Cargo.toml`, so `cargo update -p`
  alone cannot move them: bump the pin, then `cargo update -p <crate>`, and update
  `docs/dependencies.md`. A `-sys` crate (for example `libsqlite3-sys`) moves only with its
  wrapper; read the wrapper's requirement from the index cache
  (`~/.cargo/registry/index/*/.cache/<aa>/<bb>/<crate>`) and pick the smallest wrapper release
  that requires the fixed `-sys` version (rusqlite 0.41.x → libsqlite3-sys ^0.39).

## Tooling and process

- Always run `cargo clippy --color never` before grepping for `^error` or `^warning`.
- The LOC policy (`cargo run -q -p xtask -- check-loc`) caps files at 250 pure LOC, counting
  inline `#[cfg(test)]` modules. Split modules (for example `processes/stdin.rs`) instead of
  compressing code.
- On Windows, a `for` loop whose `continue` exists only under `#[cfg(unix)]` trips the denied
  `clippy::never_loop`. Move the attempt into a helper function instead. Cross-checking
  `--target aarch64-pc-windows-msvc` here fails in blake3's C build, so Windows-only code needs
  extra care.
- Web tests run under both Bun (`bun test`, inside `bun run check`) and Vitest
  (`bun run test`). `import.meta.dir` exists only in Bun; use
  `dirname(fileURLToPath(import.meta.url))`. Bun's `Request` ignores `credentials` and always
  reports `"include"`, so assert the option the client configures (for example, spy on
  `ky.create`).
- End-to-end tests run the built binaries: run `cargo build --workspace --bins` first, and do not
  rebuild while a background test run is using them.
- To split one file's changes across commits, filter the `git diff` hunks into a patch and use
  `git apply --cached --recount`. When a single hunk mixes two changes, temporarily restore one
  side in the working tree.

## Removing personal data from published history (2026-11-15)

- The repository is public. Absolute paths such as `/home/<user>/...` or an agent scratchpad
  under `/tmp/claude-<uid>/...` leak the local username. Write `~/...` or repo-relative paths in
  docs and `.omo/*`; tests use `std::env::temp_dir()`. Scan before every commit or push:
  `git grep -nIiE "$(whoami)|$(hostname -s)|claude-[0-9]+" HEAD` must print nothing.
- To scan every historical snapshot (not only diffs):
  `git rev-list --all | xargs -n 40 git grep --text -lIiE '<pattern>'`.
- Rewrite procedure that worked:
  1. Back up first with `git bundle create <outside-repo>/all-refs.bundle --all` and verify it.
     Copy untracked notes too. Snapshot `git ls-tree -r <tip>` and `rev-list --count` per
     branch.
  2. `git worktree prune`, then `pixi exec --spec git-filter-repo git-filter-repo --force
     --replace-text <file-outside-repo>`. Use literal `old==>new` rules. Keep the rules file
     out of the repo, since it contains the leaked strings.
  3. Verify that the full-history scan is empty, that commit counts are unchanged, and that the
     tip trees differ only in the sanitized files.
  4. filter-repo removes `origin`, so add it back. Look up new SHAs in
     `.git/filter-repo/commit-map`. Push with explicit leases:
     `git push origin --force-with-lease=refs/heads/B:<full-old-sha> "${NEW}:refs/heads/B"`.
- zsh pitfall: `"$VAR:refs/..."` applies the `:r` modifier. Always write `"${VAR}:refs/..."`.
  Also, `eval` strips `\b` from sed expressions, so pass mappings with `sed -f <script>`.
- Old commits stay reachable on GitHub by SHA URL after a force push until GitHub Support
  purges cached views. Other clones must be re-cloned, or they will reintroduce the old history.
