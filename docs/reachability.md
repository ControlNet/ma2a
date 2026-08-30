# Relay Reachability

MA2A supplies Iroh with a filtered relay candidate map but does not select, rank, or pin a home
relay. Iroh owns probing, effective home selection, relay connection state, and direct-path upgrades.

## Candidate Filtering

`private_relay_eligible(E, R)` means the provider and local Endpoint share at least one current
valid Space with a fresh validated advertisement for the provider and relay URL. Eligibility is
retained for target-specific dialing and does not make the relay safe for the generic map.

`home_relay_compatible(E, R)` additionally requires the local Endpoint to have at least one active
Space and the same provider and relay URL to cover every active Space. The Iroh map contains only
home-compatible private relays plus explicitly configured public fallback URLs. Zero-Space
Endpoints contribute no private candidates. MA2A never enables an implicit N0 fallback.

## Reported State

Runtime accepts an Iroh-observed home only when its URL is present in the map supplied to Iroh.
Desired configuration and candidates remain separate from effective Iroh observations in storage.
Effective observations are cleared at startup because they describe the previous process, then are
replaced as the running Endpoint reports current state.

`RelayReachability` reports:

- `NoActiveSpaces` when the Endpoint has no active Space.
- `DegradedNoCommonHome` when active Spaces have no compatible private relay and public fallback is
  disabled.
- `AwaitingIrohHome` while compatible candidates exist but Iroh has not reported a connected home.
- `IrohHomeConnected` only after Iroh reports a connected home from the supplied map.

When the candidate map or Iroh-observed `EndpointAddr` changes, Runtime preserves the Endpoint
identity and forces a newly sequenced `SpaceAddressRecord` using the live observed Endpoint data.
