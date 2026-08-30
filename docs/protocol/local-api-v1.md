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

Unknown fields, duplicate JSON object members, and unknown operations are rejected as `invalid_input`. A single numeric incompatible `version` still returns `version_mismatch` before other command validation. Duplicate `version` members are ambiguous and return `invalid_input`. Endpoint, Space, and Request identifiers use validated lowercase hexadecimal encodings of 32, 32, and 16 bytes respectively; uppercase encodings are noncanonical and rejected.

## Commands And Results

The canonical machine schema is embedded in `LOCAL_API_SCHEMA_JSON` and pinned by SHA-256 `05989fefdb0ddc90db12b89c2f20c63d21b47c19edb5dd04e84d9775d9bc31ac`. It defines every command field, nested result model, success/error response envelope, event payload, literal, nullability rule, numeric width, string bound, and collection bound. Rust tests recursively validate serialized commands, all 21 results, both response envelopes, all nine errors, and all nine events against it. Web tests use the TypeScript compiler API to recursively compare the exported command, result, response, nested model, and event types against the same schema, including primitive kinds, requiredness, nullability, literals, arrays, and references.

Successful responses contain required `version`, nullable `request_id`, `revision`, and discriminated `result` fields. Error responses contain required `version`, `error`, and nullable `remediation` fields. TypeScript exposes `LocalApiSuccessResponse`, `LocalApiErrorResponse`, and their `LocalApiResponse` union.

The `echo_call` command carries a request ID, target Endpoint ID, and UTF-8 payload of at most 4,096 bytes. Runtime routes it through the encrypted Echo v1 service without semantic retry and returns the authenticated responder Endpoint ID plus the exact echoed payload. Echo authorization failures map to `unauthorized`, concurrency saturation maps to `conflict`, timeouts and transport cancellation map to `unavailable`, and malformed protocol responses map to `invalid_input`.

The pre-authorization `handshake` result exposes only Runtime version, Endpoint ID, revision, initialization/password status, and capability flags. It contains no Space details. Client-visible values never contain key material, invitation secrets, session bearers, password verifiers, or `authorized_via` diagnostics.

## Mutation Replay

Every mutation carries a client-generated `request_id`. The Runtime fingerprints the canonical typed command after version and input validation:

- New identifier and payload: execute once.
- Same identifier and same payload: return the prior result without executing again.
- Same identifier and different payload: return `conflict` before mutation or revision read.

Durable replay storage is implemented by the Runtime persistence layer, not this contract module.

## Snapshots And Events

`snapshot_fetch` and authenticated `GET /api/v1/snapshot` return the same authoritative `RuntimeSnapshot` projection. The HTTP route returns the snapshot payload directly. Runtime/SQLite supplies the revision, verified Space membership and member counts, password state, and active-session count from one read transaction; the actor adds current Endpoint, control-sync, Iroh-observed reachability, and bounded Echo state. Until local Space labels are persisted, the canonical lowercase Space ID is the display name.

Authenticated `GET /api/v1/events?since=<revision>` accepts only a baseline equal to the current authoritative revision. A stale baseline receives one `resync-required` SSE event and the stream closes. Accepted streams use a bounded per-client queue and event IDs equal to revision. A later consecutive revision emits a typed `snapshot_invalidated` event; a skipped revision, queue overflow, Runtime unavailability/restart, or expired/revoked session emits `resync-required` when possible and closes. Polling and session-validity checks do not extend session deadlines or advance revision.

SSE events are best-effort projections, not a durable event log or replay mechanism. A disconnect, duplicate, reordered event, revision gap, `snapshot_invalidated`, or `resync-required` signal makes client state uncertain. The frontend discards further incremental events and replaces the entire state with the next successful snapshot rather than merging it with uncertain state.
