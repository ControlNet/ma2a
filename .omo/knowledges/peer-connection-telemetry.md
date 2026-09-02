# Peer connection telemetry

- `ma2a-net::ConnectionTelemetry` retains bounded observations and exposes one latest observation per remote Endpoint.
- Successful Iroh path observations include the selected path RTT in milliseconds when available.
- The Runtime actor owns a `RuntimeConnections` projection and publishes bounded `connections` entries in snapshots with `endpoint_id`, `state`, `path`, and nullable `rtt_ms`.
- `control_sync` now reports only exact synchronized peer Endpoint IDs. The misleading aggregate `synchronized` boolean is not part of command or snapshot wire output.
- Runtime Console Space rows no longer infer per-Space synchronization from a global flag. Endpoint telemetry is shown on the Endpoint page with a responsive desktop table and mobile card layout.
- Canonical local API schema hash after this contract change: `fee44b68ea82c6a8b9e8684ca90b0878b13802e251e5d7a4dac9c6d8eff7ce98`.
- Verification surfaces: `cargo test -p ma2a-net --test connection_paths`, Runtime snapshot tests, `api_schema_shapes`, strict Clippy, web TypeScript/tests/build, and Playwright desktop/mobile captures.
- `Runtime::start_with_clock` must await actor-owned initial address and relay publication before it returns. `Actor::initialize()` owns that durable readiness boundary and persists `ready=true` only after every fallible startup publication succeeds; the actor command loop and startup control round are spawned only afterward.
- The best-effort ready event is emitted at the beginning of `Actor::run()`, after successful initialization but late enough for an immediate post-start subscriber to observe it.
- A deterministic current-thread regression test must inspect the address record immediately after startup, without first sending a mailbox command, because `RuntimeHandle::status()` itself acts as an actor scheduling barrier and can hide the race.
- Connection telemetry retains failed startup dial attempts. Tests for “no existing connection” should assert that retained observations have no `last_success_at_ms`, rather than requiring the observation history to be empty.
