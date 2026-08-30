# Connectivity

MA2A delegates connection establishment and path management to Iroh. MA2A supplies an exact remote
Endpoint identity, the requested application ALPN, target-specific address data through
`SpaceAddressLookup`, and the filtered relay map described in [Relay Reachability](reachability.md).
Iroh owns DNS and socket handling, NAT traversal, relay probing and selection, path validation, and
direct-path upgrades.

## Exact-Target Dialing

`ConnectionManager::connect` accepts a `DialRequest` for one `EndpointId` and ALPN. A target-only
request is resolved by the address lookup configured on the Iroh Endpoint. An explicit
`EndpointAddr` is used only by internal callers that already obtained validated address data.

After the Iroh handshake completes, the manager verifies that the transport-authenticated remote
identity equals the requested `EndpointId`. A mismatch closes the connection and returns a permanent
authorization failure. Address records, learned paths, and relay observations never grant MA2A
authorization.

The manager owns at most 128 active connections. Shutdown cancels current dial work, closes all owned
connections, and joins the path-observation tasks.

## Recovery

Application-level recovery is finite and applies only to failures classified as transient. The
default policy permits three attempts with capped exponential delay starting at 100 ms and capped at
1.6 seconds. Each delay uses full jitter. Time, jitter, and the one-attempt Iroh adapter are injected
for deterministic tests.

Authorization, protocol-version or ALPN mismatch, revocation, malformed input, local policy denial,
and cancellation are permanent for the operation and are not retried. Both Runtime shutdown and a
request-scoped cancellation handle interrupt an in-flight attempt or retry delay. Control sync uses
one manager attempt because its existing bounded control-round recovery owns the outer retry policy.

## Observational Telemetry

`ConnectionTelemetry` records Iroh-observed state for the exact remote Endpoint:

- connecting;
- relay-only paths and the selected relay URL, when Iroh reports one;
- direct-IP-only paths;
- mixed, custom, absent, or unknown paths;
- the latest success timestamp;
- a bounded error class, detail, timestamp, and attempt count.

Telemetry retains at most eight observations for each of at most 128 remote Endpoints. Error details
are capped at 160 bytes. Runtime exposes the same bounded observations through `RuntimeConnections`.
These values are diagnostic observations, not authorization, reachability guarantees, or inputs to an
MA2A path score. Iroh path events update observations when a relay path changes or upgrades to direct.

## Live Relay Changes

`ConnectionManager::reconfigure` compares the desired filtered relay URL set with the set currently
supplied to Iroh, then calls Iroh's asynchronous `insert_relay` and `remove_relay` APIs. Iroh 1.1 does
not require Endpoint reconstruction for this operation. The manager verifies that the Endpoint is
still open and retains the same identity. Runtime then forces publication of the next actual
`SpaceAddressRecord` when the effective relay configuration changes.

## Learned Routes And Cold Start

An opportunistically learned remote route is only an Iroh performance hint. MA2A does not persist it
as an authorization fact, fabricate a route when it is absent, or assume it guarantees future
reachability. Correctness depends on current validated Space address records and relay configuration,
so connection behavior remains valid after process restart and loss of transient learned-route state.
