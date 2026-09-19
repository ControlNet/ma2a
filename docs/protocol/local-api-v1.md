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

The canonical machine schema is embedded in `LOCAL_API_SCHEMA_JSON` and pinned by SHA-256 `1add376dc7b6861e66a358cf23da99d9e96111f2da0718eab3a4dda0d404a4f8`. It defines every command field, nested result model, success/error response envelope, event payload, literal, nullability rule, numeric width, string bound, and collection bound. Rust tests recursively validate serialized commands, all 21 results, both response envelopes, all nine errors, and all nine events against it. Web tests use the TypeScript compiler API to recursively compare the exported command, result, response, nested model, and event types against the same schema, including primitive kinds, requiredness, nullability, literals, arrays, and references.

Successful responses contain required `version`, nullable `request_id`, `revision`, and discriminated `result` fields. Error responses contain required `version`, `error`, and nullable `remediation` fields. TypeScript exposes `LocalApiSuccessResponse`, `LocalApiErrorResponse`, and their `LocalApiResponse` union.

The `echo_call` command carries a request ID, target Endpoint ID, and UTF-8 payload of at most 4,096 bytes. Runtime routes it through the encrypted Echo v1 service without semantic retry and returns the authenticated responder Endpoint ID, the exact echoed payload, and the responder-reported `duration_ms` saturated at 10,000. Echo authorization failures map to `unauthorized`, concurrency saturation maps to `conflict`, timeouts and transport cancellation map to `unavailable`, and malformed protocol responses map to `invalid_input`.

The pre-authorization `handshake` result exposes only Runtime version, Endpoint ID, revision, initialization/password status, and capability flags. It contains no Space details. Client-visible values never contain key material, invitation secrets, session bearers, password verifiers, or `authorized_via` diagnostics.

## Mutation Replay

Every mutation carries a client-generated `request_id`. The Runtime fingerprints the canonical typed command after version and input validation:

- New identifier and payload: execute once.
- Same identifier and same payload: return the prior result without executing again.
- Same identifier and different payload: return `conflict` before mutation or revision read.

The Runtime persistence layer durably reserves a new identifier before execution and stores the exact encoded successful response after completion. A same-payload retry of an interrupted reservation returns `unavailable` rather than executing again; a changed payload still returns `conflict`. Failed commands that produced no successful side effect release their reservation.

Completed replay decisions are retained in deterministic FIFO order, bounded to 1,024 entries, 8 MiB of aggregate encoded responses, and 65,536 bytes per response. Oldest completed decisions are evicted by persistent sequence and Request ID until both budgets hold. Pending reservations are not evicted, so capacity exhaustion fails closed instead of permitting duplicate execution. Replay rows contain only the Request ID, canonical fingerprint, revision, encoded response, sequence, and completion state; command payloads are not persisted.

## Snapshots And Events

`snapshot_fetch` and authenticated `GET /api/v1/snapshot` return the same authoritative `RuntimeSnapshot` projection. Snapshot Spaces use the `snapshot_space` model, which carries the signed chain head the Runtime already holds: latest `generation`, `chain_hash`, the complete sorted `members` set with each member's label and capability grants, and `revoked_count`. Command results such as `space_created` keep the smaller `space` model, because a create path holds no chain head and must not invent one. Each connection carries the bounded `observations` buffer Iroh retains for that peer, at most eight entries, each with its observed path, round-trip estimate, and a closed `error_class` whose `none` member means no failure was recorded. Relay projections keep the two relay ontologies apart. `private_relay_candidates` describes MA2A Private Relays, each a role hosted by an MA2A Endpoint, and carries `provider_endpoint_id`, `relay_url`, `covered_space_ids` and the `home_compatible` verdict, so the verdict and the coverage that produced it travel together. `public_relay_fallbacks` describes external public Iroh relays, which are not MA2A Endpoints, are never Space members, and therefore carry only `relay_url`, `enabled` and `observed_connected`; no provider identity or Space coverage is synthesised for them. `reachability.state` is the Runtime's own `RelayReachability` value projected verbatim, and is the only authoritative source for it; `observed_relay_state` keeps its two facts apart by name: `private_relay_provider_running` reports whether this Runtime currently hosts its embedded Private Relay service, and `public_relay_connected` reports whether Iroh currently sees this Endpoint connected through an enabled Public Relay Fallback. Neither may be used to infer `reachability.state`. `control_rounds` retains at most sixteen completed control rounds in memory only; it is a diagnostic, is never persisted, and is empty after a restart. The HTTP route returns the snapshot payload directly. Runtime/SQLite supplies the revision, verified Space membership and member counts, and password state from one read transaction. Fields whose current source is actor-local or time-derived remain conservative constants until they are persisted under that same revision. Until local Space labels are persisted, the canonical lowercase Space ID is the display name.

Authenticated `GET /api/v1/events?since=<revision>` accepts only a baseline equal to the current authoritative revision. A stale baseline receives one `resync-required` SSE event and the stream closes. Accepted streams use a bounded per-client queue and event IDs equal to revision. A later consecutive revision emits a typed `snapshot_invalidated` event; a skipped revision, queue overflow, Runtime unavailability, or Runtime restart emits `resync-required` when possible and closes. An expired or revoked session closes immediately without a revision or state-bearing frame. Polling and session-validity checks do not extend session deadlines or advance revision.

Authenticated same-origin mutation routes require the session cookie, matching `X-CSRF-Token`, and exact `application/json`. Each route accepts one existing local API command operation and forwards the parsed typed `Command` through `LocalApiClient`; Web handlers do not mutate Runtime or SQLite state directly.

SSE events are best-effort projections, not a durable event log or replay mechanism. A disconnect, duplicate, reordered event, revision gap, `snapshot_invalidated`, or `resync-required` signal makes client state uncertain. The frontend discards further incremental events and replaces the entire state with the next successful snapshot rather than merging it with uncertain state.
