# Task 2 Core Contracts

- The Phase 1 core contract is Endpoint-centric; Space participation is represented separately by `SpaceMembership`.
- `EndpointId` validates Ed25519 public-key bytes, `SpaceId` hashes `b"ma2a-space-v1"` followed by exact genesis bytes, and `RequestId` is a random 16-byte correlation identifier.
- Wire v1 uses strict canonical CBOR with fixed field order, rejects unknown or duplicate fields, and bounds payloads at 4,096 bytes and envelopes at 4,160 bytes.
- The public API keeps closed discriminants behind invariant-preserving wrappers so workspace `clippy::exhaustive_enums` remains enabled without lint suppression.
- Verification commands: `cargo fmt --all -- --check`, `cargo clippy -p ma2a-core --all-targets --all-features -- -D warnings`, `cargo nextest run -p ma2a-core --all-features`, `cargo xtask check-pins`, `cargo xtask check-loc`, `cargo deny check`, and `cargo machete`.
