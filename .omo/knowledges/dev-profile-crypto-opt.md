# Optimized crypto crates in the dev and test profile

## Symptom

`enrollment::missing_intermediate_and_genesis_latest_only_never_validate` took 100–116 s in
debug builds. nextest kills a test at 120 s, so it timed out under load.

## Cause

`Repository::advance_owned_space` calls `load_space_chain`, which re-verifies every manifest
signature. Building a 58-generation chain is therefore quadratic in ed25519 verifies, and an
unoptimized `curve25519-dalek` takes about 19 ms per verify.

## Fix

The root `Cargo.toml` sets `opt-level = 3` for `curve25519-dalek`, `ed25519-dalek` and `sha2`
under `[profile.dev.package.*]`. The test profile inherits it. The test now takes about 6.5 s.
Workspace crates stay at `opt-level = 0`, so debugging is unaffected.

## When it recurs

If another debug-only test is slow, time its phases with `--no-capture` before blaming fsync.
The slow-fsync note in `phase-3-process-supervisor.md` was not the cause here: a `dd oflag=dsync`
write took about 5 ms.
