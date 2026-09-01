# MA2A Phase 0+1 focused plan review corrections

## Locked corrections

- Add a narrow authenticated `ma2a/control/1` protocol for bounded pull plus opportunistic push of newer signed Space manifest suffixes, current/fresh `SpaceAddressRecord`s, and current/fresh `PrivateRelayAdvertisement`s between existing members.
- Forwarded artifacts are accepted only through their original signature plus existing Space membership, generation/sequence/hash, expiry, rollback, fork, and revocation validation. Synchronization must not leak cross-Space state or introduce CRDT, consensus, durable replay, generic pub/sub, mandatory gossip, or global replication.
- MA2A computes `LocalIrohRelayMap(E)` from all home-compatible Private Relays plus enabled Public Relay fallback. Iroh performs actual preferred/home relay selection, relay connection, direct/relay path selection, and upgrades. MA2A observes the result and never adds a competing ranking/scoring algorithm.
- Embedded MA2A Private Relays may use native `iroh-relay` TLS with manually supplied certificate/key material. Compatible external TLS termination remains optional; DNS, ACME, issuance, renewal, and reachability automation remain deferred.
- Release scope remains Linux/macOS/Windows on x86_64 and ARM64 wherever the pinned stack supports them. Linux ARM64 must be included when its support probe passes; Windows ARM64 must be included when reliable build/package/smoke support passes or explicitly deferred with concrete reproducible blocker evidence.
- Control synchronization must actively resolve and dial a bounded subset of known reachable member Endpoints over `ma2a/control/1`; a pre-existing Iroh Connection is optional, never required. Startup, accepted local control-object advancement, post-enrollment, explicit CLI, and periodic reconciliation are dial triggers.
- Enrollment without a trusted checkpoint must transfer and sequentially validate/persist `SpaceGenesisV1` plus every contiguous signed manifest generation through the newly committed joining generation. Genesis+latest-only and missing intermediate generations are invalid in Phase 1.
- Endpoint-owned signed state is bound to its Endpoint signer: `SpaceAddressRecordV1` carries one outer `endpoint_id` plus identity-free Iroh EndpointData and requires signer == endpoint_id; `PrivateRelayAdvertisement` is signed by the hosting Endpoint key and requires signer == provider_endpoint_id plus existing membership/capability/revocation/sequence/expiry validation.

## Preservation rule

All other approved plan details remain unchanged unless they contradict locked MA2A semantics, omit an implementation path, silently alter behavior, or rely on an incorrect Iroh assumption. Compatible early implementation detail is not a review blocker.
