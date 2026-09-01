# Task 3 Store Schema v1

- `ma2a-store` is a synchronous `rusqlite` boundary intended for one dedicated blocking owner; every mutation uses `BEGIN IMMEDIATE` and advances one monotonic runtime revision in the same transaction.
- Startup validates owner-only state paths, rejects future schemas before changing journal mode, enables WAL, foreign keys, a five-second busy timeout, and `synchronous=FULL`, then validates schema and protected-key references.
- Schema v1 stores only opaque protected-key references in `SQLite`; private bytes live in immutable owner-only files written by same-directory temporary file, file sync, atomic no-replace hard-link publication, temporary unlink, parent sync, and post-write permission validation. The filesystem operation, rather than an in-process lock, arbitrates writers across processes.
- `KeyMaterial` groups protected-key class, reference, and borrowed secret bytes so `KeyStore::write` has a narrow typed input; reads return `ProtectedSecret`, whose allocation zeroizes on drop.
- Online backup uses the `rusqlite` backup API while the source remains open and requires destination `PRAGMA integrity_check` to return `ok`.
- Fixed schema-v1 DTOs and outcome enums intentionally remain directly constructible and exhaustively matchable; reasoned `#[expect]` attributes document these closed-contract exceptions to workspace extensibility lints.
- Verification: `cargo run --locked -p xtask -- check`, `cargo check --locked --workspace --all-targets --all-features`, `cargo build --locked --release -p ma2a-app`, and an ephemeral end-to-end store probe all pass.
- Crash recovery coverage launches the real integration-test binary as a child, mutates an open `BEGIN IMMEDIATE` transaction, signals readiness through stdout without sleeps, is forcibly killed, and then proves repository reopen rolls back both the row and runtime revision.
