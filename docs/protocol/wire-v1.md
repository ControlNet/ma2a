# MA2A Wire Version 1

Phase 1 uses one closed Endpoint-centric request/response contract. The Universe is conceptual and
has no identifier or registry. A Runtime owns one persistent Endpoint identity and stores each
`SpaceMembership(space_id, endpoint_id)` separately. Normal requests target an Endpoint. Public
envelopes never carry a Space selector or `authorized_via` value.

## Fixed Types

- `EndpointId`: exactly 32 bytes that parse as an Iroh 1.1 public key. Other lengths and invalid
  Ed25519 compressed points are rejected.
- `SpaceId`: `BLAKE3("ma2a-space-v1" || canonical_space_genesis_v1_body_cbor)`. The input is the
  exact unsigned `SpaceGenesisV1` body bytes, never its signature envelope.
- `RequestId`: 16 cryptographically random bytes from the operating system random source.
- `ProtocolVersion`: major `1`, minor `0`.
- Service kind `Echo`: integer discriminant `0`. No other service kind exists in Phase 1.
- Maximum Echo payload: 4,096 bytes.
- Maximum request or response envelope: 4,160 bytes.

## Canonical CBOR

All maps and arrays have definite lengths. Map keys are unsigned integers in ascending order. All
integers and byte-string lengths use their shortest CBOR representation. Floats, tags, text values,
indefinite values, duplicate fields, unknown fields, out-of-order fields, and trailing bytes are
invalid. Exact encoded bytes are the hash and signature boundary for later signed objects.

Decoding checks the envelope bound before reading fields and borrows byte-string payloads from the
input. An owned value is created only through an explicit `into_owned` conversion after successful
bounded parsing.

## Request Envelope

The request is a four-entry map:

| Key | Value |
| --- | --- |
| `0` | version array `[major, minor]` |
| `1` | 16-byte `RequestId` |
| `2` | 32-byte target `EndpointId` |
| `3` | operation array `[0, payload_bytes]` for Echo |

Example diagnostic form, not an alternate encoding:

```text
{
  0: [1, 0],
  1: h'000102030405060708090a0b0c0d0e0f',
  2: h'5866666666666666666666666666666666666666666666666666666666666666',
  3: [0, h'68656c6c6f']
}
```

The stable bytes are checked in as `crates/ma2a-core/testdata/wire-v1/echo-request.cbor`.

## Response Envelope

The response is a three-entry map:

| Key | Value |
| --- | --- |
| `0` | version array `[major, minor]` |
| `1` | 16-byte `RequestId` |
| `2` | result array |

Result discriminants are closed:

- Echo success: `[0, payload_bytes]`.
- Typed error: `[1, error_code]`.

Error codes are fixed:

| Code | Error |
| --- | --- |
| `0` | `VersionMismatch` |
| `1` | `InvalidInput` |
| `2` | `Unauthorized` |
| `3` | `NotFound` |
| `4` | `Conflict` |
| `5` | `Expired` |
| `6` | `Rollback` |
| `7` | `Unavailable` |
| `8` | `Internal` |

Stable success and error bytes are checked in as `echo-response.cbor` and
`error-response.cbor` beside the request vector.

## Rejection Rules

- A version other than exactly 1.0 returns `VersionMismatch`.
- Noncanonical CBOR, malformed fields, unknown/duplicate/out-of-order fields, invalid ID lengths or
  key bytes, oversized values, and trailing data return `InvalidInput`.
- Failure to obtain request randomness or reserve a bounded encoder buffer returns `Internal`.
- Decoding never panics and does not allocate based on an untrusted length.
