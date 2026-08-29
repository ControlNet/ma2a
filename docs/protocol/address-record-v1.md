# Space Address Record Version 1

`SpaceAddressRecordV1` is short-lived, Endpoint-owned reachability data scoped to one Space. It is
not a public discovery record, a relay advertisement, an Endpoint ticket, or proof that a generic
relay can reach a target. Records are forwarded unchanged only inside their signed Space.

## Canonical Encoding

The unsigned body is a seven-entry deterministic CBOR map:

| Key | Value |
| --- | --- |
| `0` | version array `[1, 0]` |
| `1` | 32-byte `SpaceId` |
| `2` | 32-byte publishing `EndpointId` |
| `3` | monotonic Endpoint-owned sequence as `u64` |
| `4` | issue time in Unix milliseconds as `u64` |
| `5` | expiry time in Unix milliseconds as `u64` |
| `6` | canonical identity-free Endpoint data array |

The signed envelope is `{0: body_bytes, 1: signature_bytes}`. The body is one CBOR byte string and
the signature is exactly 64 bytes. The complete signed envelope is at most 16,384 bytes.

Endpoint data is a two-element array `[addresses, user_data]`. `addresses` contains between 0 and 16
unique transport addresses in signed priority order. The encoder does not sort them. `user_data` is
either CBOR null for absence or UTF-8 text of at most 245 bytes; present empty text is distinct from
absence. Each transport address is a two-element array:

| Kind | Payload |
| --- | --- |
| `0` | relay URL as canonical text, at most 512 bytes |
| `1` | IPv4 octets followed by big-endian port, exactly 6 bytes |
| `2` | IPv6 octets followed by big-endian port, exactly 18 bytes |
| `3` | custom transport array `[id, data]`, where `id` is the exact `u64` transport ID and `data` is at most 1,024 opaque bytes |

Port zero, duplicate addresses, invalid relay URLs, malformed UTF-8, oversized custom or user data,
unknown transport kinds, nested Endpoint identities, unknown map fields, non-shortest CBOR,
indefinite values, and trailing bytes are invalid. Unknown future non-exhaustive Iroh transport
variants fail closed at the conversion boundary. An older wire containing an inner
`EndpointAddr.endpoint_id` is therefore rejected rather than interpreted. The 16,384-byte complete
record limit remains an independent envelope bound.

## Signature And Hash

The publishing Iroh Endpoint signs with its persistent Iroh `SecretKey`:

```text
signature = Ed25519.sign(endpoint_secret,
  "ma2a-space-address-signature-v1" || canonical_address_body)

record_hash = BLAKE3(
  "ma2a-space-address-record-hash-v1" || canonical_signed_envelope)
```

The public key selected for verification is `record.endpoint_id`. A construction boundary starting
from `EndpointAddr` must first require `endpoint_addr.id == signer.public()` and then discard the
outer identity, retaining only its transport data. Complete snapshot publication may additionally
carry Iroh `UserData`; the signed endpoint-data value itself never contains an Endpoint identity.

## Validity And Sequence

`expires_at_ms` must be greater than `issued_at_ms` and no more than 600,000 milliseconds later.
Publishers read actual current transport data from the running Iroh Endpoint. They publish sequence
zero when no prior record exists, increment the persisted sequence on material Endpoint data or
home-relay change, and refresh unchanged data after 300,000 milliseconds.

SQLite retains one high-water record per `(space_id, endpoint_id)`. A higher sequence replaces the
current record. Exact same-sequence replay is idempotent. A lower sequence is rollback, and different
hash or bytes at the same sequence is a fork. Rejected records do not change state or revision, and
the high-water decision survives Repository reopen.

## Validation Order

Untrusted records are processed in this fail-closed order:

1. Canonical CBOR, version, size, address, and validity-window bounds.
2. Verification identity selection from `record.endpoint_id` and exact requested target match.
3. Exact requested `space_id` match.
4. Current membership in that Space, with a current revocation represented by absence from the
   verified membership view.
5. Injected-clock checks: issue time is not in the future and expiry is strictly after now.
6. Endpoint Ed25519 signature verification.
7. Persistent `(space_id, endpoint_id)` sequence and hash rollback/fork check.

No rejected object is cached or forwarded.

## Private Iroh Lookup

The MA2A `AddressLookup` has no DNS or Pkarr publication path. `resolve(target)` examines only fresh,
validated records whose `endpoint_id` equals `target` and whose Space authorization view still lists
that target as a member. Each shared Space is evaluated independently in authorization snapshot
order. The lookup merges Relay, IP, and Custom addresses while retaining first-seen priority order
and removing later duplicates. Optional user data must agree exactly across all contributing Spaces,
including the distinction between absent and present empty data. The result is reconstructed with
`EndpointInfo::from_parts(target, complete_endpoint_data)`.

Missing authorization, expiry, a future-issued record, poisoned cache state, conflicting user data,
no valid target record, or any material multi-Space uncertainty returns no result. The lookup never
substitutes another member's addresses, a Space relay advertisement, locally configured public
fallback, or any synthesized identity or route.

## Metrics

Address record observability uses closed typed counters only. It counts validation acceptance and
rejection outcomes, persistence advanced/idempotent/rollback/fork outcomes, lookup future/expiry
exclusions, and cache inserted/stale/equal outcomes. The metric API has no runtime label strings and
never records Space IDs, Endpoint IDs, addresses, relay URLs, custom IDs or bytes, user data,
signatures, hashes, or other topology-bearing values.
