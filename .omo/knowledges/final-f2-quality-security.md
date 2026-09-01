# F2 quality and security review knowledge

## Release blockers

- Control protocol admission must happen before reading the bounded body. A bounded application mailbox is insufficient when each protocol handler can first allocate and retain a 1 MiB request while waiting to enqueue. Use shared global/per-peer RAII permits and adversarial saturation tests.

## Resolved findings

- Local mutation replay is persisted in SQLite schema v4. New request identifiers are durably reserved before dispatch, successful responses are replayed byte-for-byte after restart, interrupted pending requests fail closed, and changed fingerprints return `conflict`.
- Completed replay decisions use persistent FIFO eviction bounded to 1,024 entries, 8 MiB aggregate response bytes, and 65,536 bytes per response. Pending reservations are never evicted.
- The implementation landed as commits `8205cf0` through `d8c55cd`. Focused Runtime and Store nextest passed 134/134, and strict Clippy passed with warnings denied.

## Reviewed non-finding

- `ControlRoundQueue.waiters` can temporarily retain senders whose receivers were dropped, but local API/Web concurrency and the 30-second control-round lifecycle bound externally reachable accumulation; completion removes assigned waiters. This is not equivalent to the process-lifetime replay cache or pre-authorization remote control allocation.

## Audit limitations

- Team-mode security tools were unavailable; direct source tracing and three independent delegated audits were used.
- Native platform execution was Linux x86_64. Windows/macOS paths were source-reviewed and remain CI-owned.
- Rust LSP timed out; compiler, strict Clippy, and focused nextest were used as fallback diagnostics. Full `xtask check` is currently blocked at workspace rustfmt by the unrelated untracked `crates/ma2a-net/tests/control_admission.rs`.

## Inbound control admission remediation

- `ma2a/control/1` uses a two-phase call capability: Runtime receives only TLS identity first, authorizes shared-Space membership through Store, and only then authorizes transport body reading.
- `ControlLimiter` owns global and per-peer counters behind RAII permits. The production limits are 16 globally and two per peer, and permit lifetime covers authorization, bounded body read, Store response work, and transport response completion.
- `ControlMetrics` exposes active permit ownership and completed body-read counts so real-Iroh tests can synchronize on admission state without sleeps.
- Runtime keeps response-time Store authorization in addition to pre-read authorization, preserving fail-closed behavior when membership is revoked after admission.
- Real transport tests must send one probe byte before waiting for the server-side call because Iroh does not expose an empty opened stream to the peer. This does not weaken the ordering proof because the handler does not read the byte before Runtime authorization.
- Deterministic global saturation can be tested inside the handler module by pre-acquiring the production limiter and observing real-Iroh stream closure, avoiding an impractical fleet of network peers.
