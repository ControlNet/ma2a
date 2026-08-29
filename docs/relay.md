# Private and Public Relay Roles

MA2A treats private relay service and public Iroh relay fallback as separate, non-interchangeable
roles. Neither role changes the Runtime Endpoint identity or creates another Space identity.

## Private Relay Provider

`PrivateRelayProviderConfig` enables an existing Runtime Endpoint to host an embedded Iroh relay for
an explicit, nonempty set of `served_spaces`. `PrivateRelayProviderLocation` pairs the local listener
with the externally reachable HTTPS relay URL. The provider signs advertisements with the same Iroh
Endpoint secret used by the Runtime.

Admission uses the Endpoint ID authenticated by the Iroh relay handshake. Effective served Spaces
are the configured Spaces in which the hosting Endpoint is currently an unrevoked member with the
private-relay-provider capability. `PrivateRelayAccess` atomically replaces the union of members in
those effective Spaces and indexes admitted `(EndpointId, ConnectionId)` pairs. A membership update
disconnects an existing session only when the Endpoint loses eligibility through every effective
served Space; losing one of several authorizing Spaces preserves the session. Relay admission does
not replace Space or service authorization.

### Native TLS

`PrivateRelayTransport::NativeTls` loads an operator-supplied certificate chain and private key from
paths at relay startup. MA2A does not store PEM contents in configuration and does not issue, renew,
or discover certificates.

Startup fails closed unless:

- the certificate and key are opened without following links, verified from the open handles, and
  read with hard byte limits;
- every certificate is currently valid;
- the private key matches the certificate;
- on Unix, the key is owned by the effective user and has mode exactly `0600`;
- on Windows, the key owner is the current user and its protected DACL grants explicit full control
  only to that user and Local System.

The key is loaded only by the relay process, temporary key bytes are zeroized where supported, and
key material must never be logged.

### External TLS Termination

`PrivateRelayTransport::ExternalTlsTermination` starts a plaintext relay backend only on a loopback
listener. The operator-managed proxy must expose the configured HTTPS relay URL, terminate TLS, and
forward the Iroh relay HTTP and WebSocket upgrade traffic unchanged to that loopback backend.

## Space-Scoped Advertisements

`PrivateRelayAdvertisementPublisher` emits one independently signed, bounded advertisement for each
effective served Space. Before signing, Store atomically reserves the next provider-owned sequence,
so restart cannot reuse an earlier value. Each advertisement contains only its exact `space_id`,
provider Endpoint ID, HTTPS relay URL, monotonic sequence, issue time, and expiry. It never lists the
provider's other Spaces.

Consumers accept an advertisement only when canonical encoding, provider signature, exact-Space
membership, private-relay-provider capability, validity window, and persistent sequence high-water
checks all pass. Lower sequences are rollbacks; different bytes at the same sequence are forks.
Advertisements remain provider metadata and are never converted into target `EndpointData` or added
to `SpaceAddressLookup`.

## Public Iroh Relay Fallback

`PublicRelayFallbackConfig` is an explicit operator-supplied HTTPS relay set used only to construct an
Iroh `RelayMode::Custom`. Without this configuration, Runtime Endpoints use `RelayMode::Disabled`.
Public fallback entries do not become Space members, private relay advertisements, target addresses,
or authorization signals. The Runtime continues to use the MA2A private target-specific address
lookup and does not enable the unchanged N0 discovery preset.

Store persists the complete desired relay configuration: every public fallback URL, provider enable
state, listener and private HTTPS URL, exact served-Space set, transport mode, and certificate/key
paths. PEM bytes are never stored. Net parses this Store-owned record into runtime relay types.
