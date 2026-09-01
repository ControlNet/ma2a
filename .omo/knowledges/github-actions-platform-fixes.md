# GitHub Actions platform-fix knowledge

## E2E build prerequisite

- `crates/ma2a-app/build.rs` invokes Bun whenever `ma2a-app` is compiled, including `cargo test --no-run` and nextest preparation.
- Any GitHub Actions job that builds `ma2a-app` from a clean runner must install Bun before invoking Cargo.
- The CI-compatible version used by this repository is Bun `1.3.5`, installed with `oven-sh/setup-bun@v2`.
- A missing Bun prerequisite can look like an E2E test failure even though no test process, JUnit output, or JSONL evidence was created. Inspect `target/e2e-evidence/run-*.log` before diagnosing test timing.

## Windows IPC compilation

- Functions defined in the nested `windows_security::ffi` module and re-exported to the `ipc` ancestor require `pub(in super::super)`, not `pub(super)`.
- `interprocess` Tokio named-pipe streams expose a borrowed Windows handle through `AsHandle`; obtain the raw handle with `pipe.as_handle().as_raw_handle()` and import both `AsHandle` and `AsRawHandle`.
- Native Windows compilation remains the authoritative verification because Linux cross-checking the MSVC target requires tools such as `ml64.exe`.

## Apple test portability

- `rustix::fs::mknodat` is unavailable on Apple targets in the repository's rustix version.
- Gate only FIFO fixtures/tests with `#[cfg(target_os = "linux")]` or `#[cfg(all(test, target_os = "linux"))]`.
- Keep ordinary Unix permission and symbolic-link tests enabled on macOS.

## Verification surfaces

- Full repository gate: `cargo run --locked -p xtask -- check`.
- Repeated Phase One E2E gate: `./scripts/run-e2e.sh`.
- A healthy E2E summary reports three successful runs, 46 tests, and 13 JSONL records per run.
