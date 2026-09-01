# Todo 12 Findings

- Pinned Iroh 1.1 `EndpointData` is identity-free; `EndpointInfo` supplies the single target identity.
- The V1 MA2A wire carries only relay/IP `TransportAddr` data. Opaque custom transports and user data
  are rejected to keep the signed topology boundary bounded and auditable.
- Validation must use the exact current Space authorization view, never cross-Space `authorize_any`.
- Existing SQLite `address_state` already has the correct `(space_id, endpoint_id)` primary key and
  can distinguish replay, rollback, and same-sequence fork by reading hash and signed bytes.
- `RuntimeEndpoint::bind` now installs only the custom MA2A Space lookup after clearing preset lookup
  services; no DNS/Pkarr publication path is added.
- `AddressPublisher::from_endpoint` reads the running Endpoint's current `addr()` for every publish
  decision, preserving the Endpoint ID while refreshing on material data change or five minutes.
- Final verification passed with `cargo test --workspace`, `cargo build --workspace --all-targets`,
  and `cargo clippy --workspace --all-targets --all-features -- -D warnings`.
- Focused runtime checks proved live Endpoint observations are published, two shared Spaces merge
  only the requested Endpoint's addresses, and wrong-target input fails before signature/clock work.
- Independent adversarial review found and remediated three lookup-state defects: late lower-sequence
  records could replace newer cache state, a rolled-back clock could expose future-dated records,
  and duplicate authorization snapshots for one Space were order-dependent rather than uncertain.
- Regression coverage proves cache sequence monotonicity, fail-closed future-issued lookup, and
  atomic rejection of conflicting authorization views. A real two-Space Iroh fixture also resolved
  and dialed only the requested target with deduplicated addresses.
- Remediation commits are `dad558c` and `fa9a38f`. Final locked aggregate verification passed 138
  Rust tests, strict workspace Clippy, rustfmt, dependency/license/advisory checks, 41 web tests,
  all-target workspace build, no-excuse rules, and staged/tracked secret scans.

## 2026-08-30 Full EndpointData Remediation

- Replaced the relay/IP-only sorted wire with bounded identity-free Iroh 1.1 endpoint data: ordered
  unique Relay, IPv4, IPv6, and Custom addresses plus optional UTF-8 user data.
- Custom transport data is encoded as exact `u64` ID plus opaque bytes, bounded at 1,024 bytes before
  allocation/copy; user data follows Iroh's exact 245-byte limit and preserves absent versus empty.
- Lookup now traverses Spaces in authorization snapshot order, removes later duplicate addresses
  without reordering, reconstructs `EndpointInfo::from_parts(target, complete_data)`, and returns no
  result when contributing Spaces disagree on optional user data.
- Added closed typed counters for validation, persistence, lookup future/expiry exclusions, and cache
  inserted/stale/equal outcomes. The API has no runtime labels and carries no topology values.
- Red evidence: the initial full-data test failed with `ProtocolError(InvalidInput)` on Custom, and
  the initial metrics test observed accepted count `0` instead of `2` before production wiring.

## 2026-08-30T04:40:51+10:00 Live EndpointData Remediation

- Pinned Iroh 1.1 invokes `AddressLookup::publish(&EndpointData)` for the initial endpoint data and
  material address or user-data updates; this callback is the authoritative ordered live source.
- `Endpoint::addr()` is an identity-plus-address snapshot and cannot preserve callback user data, so
  live signing now reads a private latest-value observation retained by `SpaceAddressLookup`.
- The observation stores only bounded `AddressEndpointDataV1`, uses a `watch` generation signal for
  deterministic initialization, and fails closed for unavailable, invalid, poisoned, or closed state.
- Boundary conversion validates address count, relay URL length, nonzero IP ports, custom payload
  length, known transport variants, and UTF-8 user-data length before cloning callback-owned data.
- Regression coverage proves callback order and custom data preservation, user-data sequence changes,
  exact `None` versus `Some("")` semantics, and rejection of unavailable or oversized observations.
- Verification passed strict Core/Store/Net Clippy, rustfmt, 26 focused integration tests, all 29
  `ma2a-net` tests, repository LOC policy, no-excuse rules, and `git diff --check`.

## 2026-08-30T04:55:00+10:00 Live Observation Verification Corrections

- Independent review reproduced a provenance vulnerability: public code could pair Endpoint B with
  callback data injected through a separately retained `SpaceAddressLookup` clone and sign that data.
- Live construction is now crate-private and reachable publicly only through
  `RuntimeEndpoint::address_publisher`; a private runtime adapter owns callback observation while
  delegating resolution to the public `SpaceAddressLookup` shared state.
- A compile-fail doctest proves external callers cannot access the identity-free live constructor,
  and a real Iroh runtime regression proves callbacks injected through the public lookup are ignored.
- Initial callback readiness now has a two-second internal Tokio timeout. Timeout cancellation drops
  only the cancel-safe watch wait, and bind explicitly closes the Endpoint before returning the typed
  failure so no detached task or secret-owning Endpoint remains.
- Borrowed callback conversion now validates total count, every transport rule, and user-data byte
  length in a complete first pass, then clones addresses and user data only after all checks succeed.
- A deterministic paused-time test proves unavailable initialization times out without sleeps, and a
  17-address callback replaces prior ready state with invalid state rather than retaining signable data.
- Final verification passed 9 focused tests, all 30 `ma2a-net` tests, the provenance compile-fail
  doctest, rustfmt, strict Core/Store/Net Clippy, LOC, no-excuse, and `git diff --check`.

## 2026-08-30 Final Verification Correction

- The final cleanup removed an unnecessary `TestResult` return from the pure 17-address rejection
  regression and was committed as `f97b258` (`test(net): remove unnecessary conversion result`).
- Final counts are Core 9/9, Store 9/9, focused Net 26/26, full `ma2a-net` 31/31, and doctest 1/1.
  Rustfmt, strict Core/Store/Net Clippy, workspace all-target build, LOC, no-excuse across all Todo 12
  Rust files, and `git diff --check master...HEAD` also passed.
