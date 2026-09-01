# Task 12: Signed Space Address Records

## Pinned Iroh 1.1 APIs

- `iroh-dns-1.1.0/src/endpoint_info.rs`: `EndpointData` contains transport addresses and optional
  user data but no Endpoint identity. `EndpointInfo` combines one `EndpointId` with `EndpointData`.
- `iroh-base-1.1.0/src/endpoint_addr.rs`: `EndpointAddr` combines identity with a `BTreeSet` of
  `TransportAddr`; `TransportAddr` supports relay, IP, and opaque custom transports.
- `iroh-1.1.0/src/address_lookup.rs`: custom lookup returns a boxed stream of `Item`, constructed
  from `EndpointInfo::from_parts(target, data)`. An empty boxed stream is the fail-closed answer.
- `iroh-1.1.0/src/endpoint.rs`: `Endpoint::addr()` returns the current observed/configured address,
  and `Endpoint::secret_key()` provides the Endpoint signer without exporting private bytes.

## MA2A Boundary Decisions

- Canonical `SpaceAddressRecordV1` carries one outer Endpoint identity and identity-free transport
  data only. V1 permits relay and IP addresses, rejects custom transports and user data, and therefore
  rejects legacy nested Endpoint identities as unknown/noncanonical bytes.
- Signatures and record hashes use separate domains. Parsing is separate from signature verification
  so network validation can enforce target, Space, membership, and clock ordering before persistence.
- Current membership is checked against one exact `SpaceAuthorizationView`; `authorize_any` is not
  valid for accepting Space-local address state.
- SQLite's existing `(space_id, endpoint_id)` primary key is sufficient. Same bytes at the same
  sequence are idempotent, lower sequence is rollback, and different bytes/hash at the same sequence
  are a fork across Repository reopen.
- The private `AddressLookup` evaluates each shared Space independently, merges only records whose
  signed Endpoint equals the requested target, and returns no result on missing authorization,
  expiry, revocation, poisoned state, or empty data.

## Verification Surfaces

- Core deterministic canonical/signature and malformed/legacy tests.
- Real SQLite reopen tests for idempotence, rollback, and same-sequence fork rejection.
- Ordered validation tests for target precedence, membership, clock, and signature.
- Snapshot and live running-Endpoint publisher tests for identity binding, material changes, and
  five-minute refresh with persisted monotonic sequence.
- Multi-Space target-only lookup and Space-local revocation removal tests.

## Final Implementation Notes

- `AddressRecordScope` groups the exact `(SpaceId, EndpointId)` subject and
  `AddressRecordValidity` makes the monotonic sequence plus bounded lifetime valid before a
  `SpaceAddressRecordV1` can exist.
- Runtime binding uses `presets::Minimal`, disables relays, clears preset lookup services, and
  installs only `SpaceAddressLookup`; the sole discovery text match is documentation of this
  disabled-public-discovery boundary.
- Lock guards in synchronous lookup are released before constructing the Iroh result, avoiding
  unnecessary contention while preserving a fail-closed empty result on poisoned state.
- Full workspace tests, all-target build, strict all-feature workspace clippy, formatting, and the
  Rust no-excuse checker passed on 2026-08-29. Focused live Endpoint publication, target-only
  multi-Space lookup, and wrong-target rejection scenarios also passed.
