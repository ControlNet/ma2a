# Single-Owner Space Version 1

Phase 1 has no authority transfer. The genesis `initial_member` is the owner Endpoint.
For a locally owned Space, `Repository::advance_owned_space()` rejects any proposal
that omits that Endpoint from members or includes it in revocations, before signing
or persistence. Owner-side member removal returns an owner-specific unauthorized
response; the owner also cannot leave. Generic signed-chain validation is unchanged:
this local signing restriction does not redefine the validity of imported chains.


Phase 1 Spaces have one Ed25519 authority. The creating Repository generates a fresh authority seed
and genesis nonce, publishes the seed through the protected `KeyStore`, and stores only an opaque
reference in SQLite. Endpoint keys are never reused as Space authority keys. Imported Spaces contain
no local authority reference. Genesis is invalid when its authority public key equals the initial
member Endpoint public key.

## Canonical Encoding

Every object uses deterministic CBOR:

- Definite map, array, byte-string, and text lengths only.
- Unsigned integer map keys in ascending order.
- Shortest integer and length encoding only.
- No tags, floats, indefinite values, duplicate keys, unknown keys, or trailing bytes.
- Members and revocations are strictly sorted by raw 32-byte `EndpointId`; duplicates are invalid.

The signed envelope for genesis and manifests is the two-entry map below. `body` is encoded as one
CBOR byte string, not embedded as a nested CBOR value.

| Key | Value |
| --- | --- |
| `0` | exact canonical body bytes as a byte string |
| `1` | 64-byte Ed25519 signature |

## Genesis Body

`SpaceGenesisV1` is a six- or seven-entry map:

| Key | Value |
| --- | --- |
| `0` | version array `[1, 0]` |
| `1` | 32-byte random nonce |
| `2` | creation time in Unix milliseconds as `u64` |
| `3` | 32-byte Ed25519 authority public key |
| `4` | initial member record |
| `5` | Space policy |
| `6` | shared Space name, present only in a seven-entry body |

The initial member record is `{0: endpoint_id, 1: label, 2: capability_bits}`. The policy is
`{0: echo_enabled, 1: private_relay_provider_enabled, 2: maximum_members}`. Capability bit `0`
grants Echo and bit `1` grants private-relay-provider service.

A member label names an Endpoint inside the Space and is a different signed fact from the Space
name in key `6`; neither is ever written in place of the other, so a Space never appears in a
member list. Phase 1 has no user-facing Endpoint naming, so both Space creation and enrollment
derive the label from the member's own Endpoint identity, which keeps it stable, unique per
Endpoint and impossible to confuse with a Space name. Nothing writes a placeholder label standing
in for a person.

The shared Space name is 1–128 UTF-8 bytes with no control characters. It is signed with the rest
of genesis and therefore enters the derived `SpaceId`, so every enrolled member reads exactly the
same name from the chain it already verifies; no member configures a local alias. Space names are
deliberately not unique — two Spaces may share a name because their Space IDs differ. Key `6` is
the only optional entry, and a body that omits it encodes byte-for-byte as it did before Space
names existed, so Spaces created earlier stay valid and simply carry no name.

The genesis signature is:

```text
Ed25519.sign(authority_secret,
  "ma2a-space-genesis-signature-v1" || canonical_genesis_body)
```

The Space identifier is derived from the unsigned body only:

```text
SpaceId = BLAKE3("ma2a-space-v1" || canonical_genesis_body)
```

Signature bytes, envelope headers, and any local key reference are not `SpaceId` input. The first
manifest links to:

```text
BLAKE3("ma2a-space-genesis-chain-hash-v1" || canonical_signed_genesis_envelope)
```

## Manifest Body

`SpaceManifestV1` is a seven-entry map:

| Key | Value |
| --- | --- |
| `0` | version array `[1, 0]` |
| `1` | 32-byte `SpaceId` |
| `2` | generation as `u64`, beginning at `1` |
| `3` | 32-byte previous chain hash |
| `4` | issue time in Unix milliseconds as `u64` |
| `5` | complete sorted current member array |
| `6` | complete sorted current endpoint-revocation array |

The manifest signature and chain hash use separate domains:

```text
signature = Ed25519.sign(authority_secret,
  "ma2a-space-manifest-signature-v1" || canonical_manifest_body)

manifest_hash = BLAKE3(
  "ma2a-space-manifest-hash-v1" || canonical_signed_manifest_envelope)
```

Every accepted manifest must use the genesis authority, match the Space ID, advance by exactly one
generation, and name the current chain hash as `previous_hash`. Exact replay of the current signed
manifest is idempotent. An older generation is rollback. A divergent current generation, wrong
Space, wrong link, or divergent prefix is a fork. A skipped generation is invalid.

Removing a member requires the same generation to contain that endpoint in `revocations`. Every
later manifest repeats the revocation while the endpoint remains absent. A revocation may disappear
only when that same valid linked manifest explicitly re-adds the endpoint to the complete member set.
Revoking an endpoint absent from both prior members and prior revocations is invalid. Revocation is
scoped to one Space; there is no global revocation or lease expiry.

## Persistence And Import

SQLite commits genesis, every signed manifest, highest generation/hash, current members,
revocations, and one monotonic Repository revision in one immediate transaction. Owned advancement
loads the protected authority seed into a zeroizing buffer, verifies it against genesis, signs the
next exact generation/hash link, and derives authorization only after the transaction commits.
Rejected rollback, fork, skip, or persistence failures do not change rows or revision.

Public export contains only the signed genesis and ordered signed manifests. Import parses and
validates the complete chain before opening persistence, then commits the complete accepted state in
one transaction. It never imports, exports, or synthesizes an authority secret or opaque key
reference. Repository reopen verifies the complete signed chain, every stored generation/link/hash
row, the genesis-derived `SpaceId`, materialized member/revocation state, highest generation/hash,
and any local protected authority seed against the genesis public authority. Legacy raw Space and
manifest persistence entry points parse and verify their signed bytes before they can mutate state.

The portable public-chain envelope is `{0: signed_genesis_bytes, 1: [signed_manifest_bytes...]}`.
Each signed object is at most 32,768 bytes. A portable chain contains at most 255 manifests and is at
most 8,389,381 bytes including canonical CBOR framing. Export rejects a chain beyond those bounds;
every successfully exported chain is accepted by the same-version importer when its signatures and
transitions remain valid.

## Golden Vector

The committed vector uses authority seed `11` repeated 32 times and nonce `33` repeated 32 times.
The seed is test-only and never enters SQLite or public export.

| Artifact | Bytes | SHA-256 |
| --- | ---: | --- |
| `genesis-body.cbor` | 146 | `f07b0f99ea347f3cd9b937f676ed9388f149cfe6ac86ece04ab1728e15b9a550` |
| `genesis.cbor` | 217 | `550082f23f913454c39fd04ff55998c352aec6000876e95acfd315eb761f2939` |
| `manifest-1.cbor` | 264 | `9f27c7fc9ded273a64b463ce548793be44baa04960bab5315adc66984f2f9d41` |

Expected values:

```text
SpaceId: 041975da857a74bfc734ab39095071404f17c80f79c597047ddcbfa6e7eec0d3
Genesis signature: 7aae8329b84a796daa3cef3b6e0e4c992277c2411445b2609fdb997851973bbbce49d1c54ad49c13905c74f03566dd644128b5f01eaee92f5daaae17c13f8a08
Manifest 1 signature: 3b13865957431058fadbb7531dc138aabf8b2cb8e48486150ad929c4544ce1ed99948bb2f7f64f524ee6962e23ff42cfa9924ab2035145c257c3d11d619ac00e
Manifest 1 hash: 22ff84512151807e770630d00a003eb05fb25909c04ff9eb7f1f2902f163a69f
```

The CBOR files are the normative bytes. Hex text files beside them pin the derived values. The
manifest signature was independently generated with OpenSSL Ed25519 over the documented domain and
body bytes; hashes were generated by a standalone BLAKE3 driver that imports no MA2A production
module.

## Historical locally owned inconsistency

A Repository holding an authority reference rejects a chain whose current members
omit the genesis owner or whose current revocations include that owner. This check
runs when loading a chain (including Repository opening and authority operations)
and before replacing its durable rows, including control-batch imports. Such state
is an integrity error, not a transient maintenance failure. The Repository does
not sign a repair generation, remove the key reference, or delete protected keys.
An operator must preserve the state directory and arrange explicit recovery before
that Repository can run again; there is no automatic migration for these chains.
Public chains without a local authority reference retain generic Core validity.
