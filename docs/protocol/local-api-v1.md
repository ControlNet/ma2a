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

The canonical machine schema is embedded in `LOCAL_API_SCHEMA_JSON` and pinned by SHA-256 `f09b77a5d1809d93809be557ac1554fe06b5e7304d38b497a4886b0cd330497e`. It defines every command field, nested result model, success/error response envelope, event payload, literal, nullability rule, numeric width, string bound, and collection bound. Rust tests recursively validate serialized commands, all 26 results, both response envelopes, all nine errors, and all nine events against it. Web tests use the TypeScript compiler API to recursively compare the exported command, result, response, nested model, and event types against the same schema, including primitive kinds, requiredness, nullability, literals, arrays, and references.

Successful responses contain required `version`, nullable `request_id`, `revision`, and discriminated `result` fields. Error responses contain required `version`, `error`, and nullable `remediation` fields. TypeScript exposes `LocalApiSuccessResponse`, `LocalApiErrorResponse`, and their `LocalApiResponse` union.

The `echo_call` command carries a request ID, target Endpoint ID, and UTF-8 payload of at most 4,096 bytes. Runtime routes it through the encrypted Echo v1 service without semantic retry and returns the authenticated responder Endpoint ID, the exact echoed payload, and the responder-reported `duration_ms` saturated at 10,000. Echo authorization failures map to `unauthorized`, concurrency saturation maps to `conflict`, timeouts and transport cancellation map to `unavailable`, and malformed protocol responses map to `invalid_input`.

The pre-authorization `handshake` result exposes only Runtime version, Endpoint ID, revision, initialization/password status, and capability flags. It contains no Space details. Client-visible values never contain key material, invitation secrets, session bearers, password verifiers, or `authorized_via` diagnostics.

`space_leave` carries a request ID and a `space_id` and returns `space_left`, whose payload is the
`space_identity` model: the Space's `space_id` and shared `name` only. It deliberately carries no
`member_count`, because after departure this Endpoint holds no authoritative membership for that
Space and reporting the pre-departure count would present stale state as current. It is a membership operation, not a local deletion: the leaving member sends an authenticated request over the enrollment ALPN to the Space authority, which signs the next manifest generation removing that member and returns the advanced signed chain. The leaving Runtime persists only a chain whose signed membership excludes it and whose revocations include it. A Space's own authority-holding Endpoint cannot leave and receives `unauthorized` with remediation. An unreachable authority returns `unavailable`; nothing local is removed.

`ui_init` atomically establishes or replaces the Web password and revokes existing sessions. The public CLI uses this operation for both first setup and recovery. Lower-level `ui_password_set` and `ui_password_reset` operations retain their explicit preconditions for local API callers.

`ui_start` carries `host` (an IP address or hostname) and `port` (0–65535; zero selects a free port). `ui_stop` and `ui_status` carry no fields. All three return `ui_status` with `running` and nullable `url`. These are private IPC lifecycle instructions, have no `request_id`, execute on every call, and never participate in durable replay. Each successful `ui_start` restarts an existing service. A new daemon always starts with WebUI stopped. HTTP routes do not expose UI initialization or lifecycle control.

## Mutation Replay

Every durable mutation carries a client-generated `request_id`. The Runtime fingerprints the canonical typed command after version and input validation:

- New identifier and payload: execute once.
- Same identifier and same payload: return the prior result without executing again.
- Same identifier and different payload: return `conflict` before mutation or revision read.

The Runtime persistence layer durably reserves a new identifier before execution and stores the exact encoded successful response after completion. A same-payload retry of an interrupted reservation returns `unavailable` rather than executing again; a changed payload still returns `conflict`. Failed commands that produced no successful side effect release their reservation.

Completed replay decisions are retained in deterministic FIFO order, bounded to 1,024 entries, 8 MiB of aggregate encoded responses, and 65,536 bytes per response. Oldest completed decisions are evicted by persistent sequence and Request ID until both budgets hold. Pending reservations are not evicted, so capacity exhaustion fails closed instead of permitting duplicate execution. Replay rows contain only the Request ID, canonical fingerprint, revision, encoded response, sequence, and completion state; command payloads are not persisted.

## Snapshots And Events

`snapshot_fetch` and authenticated `GET /api/v1/snapshot` return the same authoritative `RuntimeSnapshot` projection. Snapshot Spaces use the `snapshot_space` model, which carries the signed chain head the Runtime already holds: latest `generation`, `chain_hash`, `member_count`, and `revoked_count`. The global snapshot carries no member arrays. Command results such as `space_created` keep the smaller `space` model, because a create path holds no chain head and must not invent one. Each connection carries the bounded `observations` buffer Iroh retains for that peer, at most eight entries, each with its observed path, round-trip estimate, and a closed `error_class` whose `none` member means no failure was recorded. Relay projections keep the two relay ontologies apart. `private_relay_candidates` describes MA2A Private Relays, each a role hosted by an MA2A Endpoint, and carries `provider_endpoint_id`, `relay_url`, `covered_space_ids` and the `home_compatible` verdict, so the verdict and the coverage that produced it travel together. `public_relay_fallbacks` describes external public Iroh relays, which are not MA2A Endpoints, are never Space members, and therefore carry only `relay_url`, `enabled` and `observed_connected`; no provider identity or Space coverage is synthesised for them. `reachability.state` is the Runtime's own `RelayReachability` value projected verbatim, and is the only authoritative source for it; `observed_relay_state` keeps its two facts apart by name: `private_relay_provider_running` reports whether this Runtime currently hosts its embedded Private Relay service, and `public_relay_connected` reports whether Iroh currently sees this Endpoint connected through an enabled Public Relay Fallback. Neither may be used to infer `reachability.state`. `control_rounds` retains at most sixteen completed control rounds in memory only; it is a diagnostic, is never persisted, and is empty after a restart. The HTTP route returns the snapshot payload directly. Runtime/SQLite supplies the revision, verified Space membership and member counts, and password state from one read transaction. Fields whose current source is actor-local or time-derived remain conservative constants until they are persisted under that same revision. A Space `name` is the shared name signed into that Space's genesis body, so every member projects the same value; a Space created before genesis carried a name has no persisted name and falls back to its canonical lowercase Space ID.

`space_details_fetch` takes a `space_id` and returns `space_details`: a `revision`, a `space` summary, and the complete sorted `members` set with labels and capability grants (at most 64). These fields come from one SQLite read transaction for that Space. A missing Space or one whose signed membership no longer includes the local Endpoint returns `not_found`. Authenticated `GET /api/v1/spaces/{space_id}` forwards this query through IPC and returns its payload; it maps `not_found` to HTTP 404. It does not build a global snapshot.

The frontend caches details by Space ID and chain hash, loads at most four details concurrently, and distinguishes pending or failed reads from an empty membership set. Spaces, Peers and Map share this cache. Changed heads and removed Spaces invalidate cached details; disconnect/resync clears the cache and invalidates pending responses. Unrelated revisions retain matching details. A detail whose head differs from the current summary cannot be installed.

`snapshot_stamp` returns only the durable `revision` and this process's `runtime_boot_id`. SSE uses this lightweight IPC query for baseline and polling, without constructing a full snapshot. Its revision comes from the Store so independent authentication mutations remain observable. The success envelope revision equals the payload revision for snapshots, details and stamps.

This exact v1 schema update requires coordinated Runtime and frontend upgrades. The 65,536-byte response bound remains unchanged, including the IPC snapshot boot stamp. Splitting members removes the Space-count × member-count amplification; other aggregate collections (connection histories and relay coverage) remain subject to the total byte bound. Their individual item limits do not guarantee that every combination fits; general collection pagination is outside this change.

Authenticated `GET /api/v1/events?since=<revision>` accepts only a baseline equal to the current authoritative revision. A stale baseline receives one `resync-required` SSE event and the stream closes. Accepted streams use a bounded per-client queue and event IDs equal to revision. A later consecutive revision emits a typed `snapshot_invalidated` event; a skipped revision, queue overflow, Runtime unavailability, or Runtime restart emits `resync-required` when possible and closes. An expired or revoked session closes immediately without a revision or state-bearing frame. Polling and session-validity checks do not extend session deadlines or advance revision. Sliding an active session does not advance the revision either: `last_seen_at_ms` and `idle_expires_at_ms` reach no snapshot projection, so a slide is not observable state. Advancing for it would make every authenticated read a state change and let the console's own reads drive an endless invalidate-and-refetch cycle. Session creation, deletion and revocation still advance, because they change the `active_sessions` count the snapshot carries.

Authenticated same-origin mutation routes require the session cookie, matching `X-CSRF-Token`, and exact `application/json`. Each route accepts one existing local API command operation and forwards the parsed typed `Command` through `LocalApiClient`; Web handlers do not mutate Runtime or SQLite state directly.

SSE events are best-effort projections, not a durable event log or replay mechanism. A disconnect, duplicate, reordered event, revision gap, `snapshot_invalidated`, or `resync-required` signal makes client state uncertain. The frontend discards further incremental events and replaces the entire state with the next successful snapshot rather than merging it with uncertain state.
