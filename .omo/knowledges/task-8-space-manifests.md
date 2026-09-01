# Task 8 Space Manifest Knowledge

- `SpaceId` hashes `b"ma2a-space-v1"` followed by the exact canonical unsigned genesis body. The
  signed genesis envelope is used only for the separately domain-separated generation-zero chain
  hash.
- Manifests carry complete sorted current member and Space-local revocation sets. A prior revocation
  must remain present while the endpoint is absent; removing it is valid only when the same linked
  manifest re-adds that endpoint as a member.
- Authorization is an immutable projection of a verified `SpaceChain`; it has no wall-clock lease.
  Cached last-valid views survive partition, while a newly committed revocation changes only its
  Space.
- Store publication order is validate candidate, open `BEGIN IMMEDIATE`, replace signed chain and
  derived rows, increment Runtime revision, commit, then derive/expose authorization.
- Reopen is a high-water validation boundary: verify the derived Space ID, generation-zero row,
  every signed manifest and stored metadata tuple, latest generation/hash, materialized rows, and
  any protected authority reference/public-key binding.
- Portable export is exactly signed public genesis plus the contiguous ordered signed manifest
  chain. Authority seeds and opaque local key references never enter the encoding.
- Test fixtures use deterministic non-production seeds only in test code; real owned-Space tests use
  OS-random authority material persisted through the owner-only protected KeyStore.
