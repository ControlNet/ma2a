# Task 16: Iroh Connection Management

- Iroh 1.1 supports live asynchronous relay mutation through `Endpoint::insert_relay` and
  `Endpoint::remove_relay`; Endpoint reconstruction is unnecessary for relay-map changes.
- Keep target-only dialing authoritative through the Endpoint's configured `SpaceAddressLookup`.
  Explicit `EndpointAddr` dialing is appropriate only after a caller already has validated address
  data.
- Verify `Connection::remote_id()` against the requested `EndpointId` after the handshake. Iroh path
  and relay observations remain diagnostics and never become authorization.
- Iroh exposes current paths through `Connection::paths()` and changes through `path_events()`. Map
  only observed transport composition into bounded `connecting`, `relay`, `direct`, or
  `mixed/unknown` telemetry; do not add MA2A path scoring.
- Recovery should be one finite layer. The generic manager retries only transient failures with
  capped exponential full jitter and cancellation. Existing control-round recovery uses the
  manager's single-attempt seam to avoid nested retry multiplication.
- Bound both ownership and diagnostics: this implementation caps active connections at 128, remote
  telemetry keys at 128, history at eight observations per remote, and error details at 160 bytes.
- Learned routes are performance hints only. Acceptance behavior must still work after restart and
  loss of transient Iroh route state.
- Iroh's local test relay requires the test Endpoint builder to use the test CA configuration; with
  a relay-only target record, the connection first exposes a relay path and then selects a direct IP
  path while the relay path may remain open.
- Runtime relay-candidate refresh owns forced Todo 12 publication. Prove it through
  `Actor::refresh_relay_candidates` and the persisted record sequence, not by manually forcing the
  publisher from the test.
- For bounded UTF-8 error details, cap the candidate byte index with `min(limit)`, move backward until
  `str::is_char_boundary` is true, and slice before allocating. Never call `String::truncate` at a
  presumed byte limit unless that index has first been proven to be a character boundary; this keeps
  retained text valid UTF-8 without lossy conversion or panics.
