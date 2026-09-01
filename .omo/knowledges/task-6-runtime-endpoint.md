# Task 6 Runtime Endpoint

- Persist one Iroh private key through the protected key store and keep only its opaque reference in
  SQLite. Derive the public Endpoint ID from protected bytes on every start and compare it with the
  persisted public record before binding transport.
- Use `iroh::endpoint::presets::Minimal` as a narrow base, then explicitly disable relays, clear
  address lookup, install only a private `MemoryLookup`, and register only the enrollment ALPN.
- A synchronous SQLite repository and protected key store can remain single-owner without blocking
  Tokio workers by running a bounded `mpsc` command loop in one `spawn_blocking` task.
- Runtime state changes should flow through one bounded actor. Membership replacement updates the
  persisted observation and monotonic revision while the long-lived Endpoint remains untouched.
- Zero-Space isolation is strongest at ALPN negotiation: omit normal protocols from the Endpoint's
  accepted ALPN list so payload parsing is never reached.
- Deterministic shutdown evidence should include Iroh closure, final persisted revision, and the
  exact number of joined actor/blocking tasks.
- Verification command: `cargo run --locked -p xtask -- check`.
- Actor state is authoritative only after persistence succeeds: build a typed candidate snapshot,
  persist it, attach the returned revision, then replace the published state and emit its event.
- Cancellation shutdown must acknowledge the revision returned by the final not-ready Endpoint
  observation; reusing the prior state revision produces stale deterministic shutdown evidence.
