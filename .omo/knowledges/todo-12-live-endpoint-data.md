# Todo 12 Live EndpointData

## Authoritative Source

For pinned Iroh 1.1, `AddressLookup::publish(&EndpointData)` is the complete live observation source.
Iroh calls it after initial endpoint discovery and when address or user-data state materially changes.
`Endpoint::addr()` does not carry `UserData`, so it must not be used to construct signed live records.

## Implementation Pattern

- Keep the observation type crate-private and cloneable through shared state.
- Convert borrowed callback data into bounded `AddressEndpointDataV1` before retaining it.
- Store only the latest value; use a `tokio::sync::watch` generation to signal initialization/changes.
- Preserve callback address order, opaque custom transports, and exact optional user-data semantics.
- Fail closed on unavailable, invalid, poisoned, or closed observation state.
- Keep live publisher construction crate-private and expose it only through
  `RuntimeEndpoint::address_publisher`; identity-free observations must never be accepted alongside an
  independently supplied signer at a public API boundary.
- Install a crate-private runtime lookup adapter that owns the callback observation and delegates only
  resolution to public `SpaceAddressLookup` state. Retained public lookup clones cannot inject live
  signer observations.
- Bound initial callback readiness to two seconds for local Iroh initialization. Tokio timeout drops
  the cancel-safe watch wait, and bind closes the Endpoint before returning timeout/failure.
- Validate all borrowed count, transport, and user-data bounds before cloning any callback value.

## Verification Signals

- Live metadata is present in the signed record.
- Changing user data advances the sequence.
- `None`, `Some("")`, and later `None` remain distinct material states.
- Ordered Relay/IP/Custom callback data survives signing unchanged.
- Oversized callback data invalidates the latest observation without retaining signable data.
- A 17-address callback invalidates prior ready state rather than retaining signable data.
- Initialization waits on callback notification with a deterministic bounded timeout, never sleeps or
  polling.
- External construction of a live publisher fails to compile, and callbacks injected through retained
  public lookup clones do not affect the Runtime-owned signed observation.
