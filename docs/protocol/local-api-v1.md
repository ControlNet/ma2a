# Local Runtime API v1

The local Runtime API is transport-neutral. IPC and loopback HTTP adapters carry the same bounded JSON messages and must not alter their semantics.

## Versioning

Every command carries `version: 1`. The Runtime reads and validates that field before operation parsing, replay lookup, state reads, or mutation. Any other value returns `version_mismatch` with remediation to use a client and Runtime that both implement version 1. Phase 1 performs no compatibility negotiation.

## Bounds

- Request: 16,384 encoded bytes
- Response: 65,536 encoded bytes
- SSE event data: 16,384 encoded bytes
- General text: 4,096 UTF-8 bytes, with narrower field-specific limits
- Collection: 256 entities

Unknown fields and operations are rejected as `invalid_input`. Endpoint, Space, and Request identifiers use validated lowercase hexadecimal encodings of 32, 32, and 16 bytes respectively.

## Commands And Results

The canonical closed inventories are embedded in `LOCAL_API_SCHEMA_JSON` and pinned by SHA-256 `6703648a604f92cf5011b449ca42d4242fa9c6453b5ecf3a94c98ef98009dcf6`. Rust and TypeScript consumers exhaustively match every command, result, error, and event discriminant.

The pre-authorization `handshake` result exposes only Runtime version, Endpoint ID, revision, initialization/password status, and capability flags. It contains no Space details. Client-visible values never contain key material, invitation secrets, session bearers, password verifiers, or `authorized_via` diagnostics.

## Mutation Replay

Every mutation carries a client-generated `request_id`. The Runtime fingerprints the canonical typed command after version and input validation:

- New identifier and payload: execute once.
- Same identifier and same payload: return the prior result without executing again.
- Same identifier and different payload: return `conflict` before mutation or revision read.

Durable replay storage is implemented by the Runtime persistence layer, not this contract module.

## Snapshots And Events

`RuntimeSnapshot` is authoritative and includes revision, Endpoint, Spaces, control synchronization, relay candidates, observed relay state, reachability, recent Echo summary, and UI authentication state.

SSE events are best-effort projections containing a revision and changed entity IDs. They are not a durable event log. A disconnect, duplicate, reordered event, revision gap, or `snapshot_invalidated` event requires fetching a new authoritative snapshot before applying further incremental events.
