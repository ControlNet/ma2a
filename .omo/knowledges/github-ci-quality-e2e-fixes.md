# GitHub CI quality and E2E fixes

## Failures diagnosed

- The quality workflow enforces a 250 pure line limit per Rust source file. API command methods, command parsing, IPC interaction handlers, and SSE event production exceeded it after the daemon and UI lifecycle work.
- `scripts/run-e2e.sh` expected 46 tests although the E2E binary now contains 51.
- Scenario F evidence records the second manual publication sequence separately from the owner Runtime high water. Runtime startup publishes one additional address record, so the durable receiver high water must equal `owner_runtime_high_water` and each value must be `record_sequences_after + 1`.
- The embedded production UI test still assumed daemon startup opened the WebUI automatically.
- The API schema shape validator did not implement the new `ip_or_hostname` string format.
- `rustls` 0.23.43 is affected by RUSTSEC-2026-0285; 0.23.45 contains the fix.

## Stable verification

- `cargo run --locked -p xtask -- check-loc` checks the Rust source line policy.
- `cargo deny check`, `cargo-machete`, `bun x biome check .`, `bun x tsc --noEmit`, and `bun test` cover the quality stages after Rust tests.
- `MA2A_E2E_RUNS=1 MA2A_E2E_OUTPUT=target/e2e-evidence-local ./scripts/run-e2e.sh` must finish with 51 tests and 13 evidence records per run.
- Repeated full workspace runs can leave detached test daemons after a failed lifecycle test. Before diagnosing later timeout noise, inspect processes by state directory and stop only `/tmp/ma2a-*` test daemons. Do not stop the user's normal state directory daemon.
