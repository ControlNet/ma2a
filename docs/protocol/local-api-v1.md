# Local Runtime API v1

The local Runtime API is transport-neutral. IPC and loopback HTTP adapters carry the same bounded JSON messages and must not alter their semantics.

## Versioning

Every command carries `version: 1`. The Runtime reads and validates that field before operation parsing, replay lookup, state reads, or mutation. Any other value returns `version_mismatch` with remediation to use a client and Runtime that both implement version 1. Phase 1 performs no compatibility negotiation.

## Bounds

- Request: 16,384 encoded bytes
- Response frame: 65,536 encoded bytes
- SSE event data: 16,384 encoded bytes
- General text: 4,096 UTF-8 bytes, with narrower field-specific limits
- Ordinary command/event collection: 256 entities; complete snapshot collections stream without truncation

Unknown fields, duplicate JSON object members, and unknown operations are rejected as `invalid_input`. A single numeric incompatible `version` still returns `version_mismatch` before other command validation. Duplicate `version` members are ambiguous and return `invalid_input`. Endpoint, Space, and Request identifiers use validated lowercase hexadecimal encodings of 32, 32, and 16 bytes respectively; uppercase encodings are noncanonical and rejected.

## Commands And Results

The canonical machine schema is embedded in `LOCAL_API_SCHEMA_JSON` and pinned by SHA-256 `110f4ca3729df0b3f5236c33db6420ff4d79ec80c69c08e2f4af07cdc0f9a587`. It defines every command field, nested result model, success/error response envelope, event payload, literal, nullability rule, numeric width, string bound, and collection bound. Rust tests recursively validate serialized commands, all 55 results, both response envelopes, all nine errors, and all nine events against it. Web tests use the TypeScript compiler API to recursively compare the exported command, result, response, nested model, and event types against the same schema, including primitive kinds, requiredness, nullability, literals, arrays, and references.

Successful responses contain required `version`, nullable `request_id`, `revision`, and discriminated `result` fields. Error responses contain required `version`, `error`, and nullable `remediation` fields. TypeScript exposes `LocalApiSuccessResponse`, `LocalApiErrorResponse`, and their `LocalApiResponse` union.

The `echo_call` command carries a request ID, target Endpoint ID, and UTF-8 payload of at most 4,096 bytes. Runtime routes it through the encrypted Echo v1 service without semantic retry and returns the authenticated responder Endpoint ID, the exact echoed payload, and the responder-reported `duration_ms` saturated at 10,000. Echo authorization failures map to `unauthorized`, concurrency saturation maps to `conflict`, timeouts and transport cancellation map to `unavailable`, and malformed protocol responses map to `invalid_input`.

The pre-authorization `handshake` result exposes only Runtime version, Endpoint ID, revision, initialization/password status, and capability flags. It contains no Space details. Client-visible values never contain key material, invitation secrets, session bearers, password verifiers, or `authorized_via` diagnostics.

`space_leave` carries a request ID and a `space_id` and returns `space_left`, whose payload is the
`space_identity` model: the Space's `space_id` and shared `name` only. It deliberately carries no
`member_count`, because after departure this Endpoint holds no authoritative membership for that
Space and reporting the pre-departure count would present stale state as current. It is a membership operation, not a local deletion: the leaving member sends an authenticated request over the enrollment ALPN to the Space authority, which signs the next manifest generation removing that member and returns the advanced signed chain. The leaving Runtime persists only a chain whose signed membership excludes it and whose revocations include it. A Space's own authority-holding Endpoint cannot leave and receives `unauthorized` with remediation. An unreachable authority returns `unavailable`; nothing local is removed. A second `space_leave` for a Space whose departure is still waiting on the authority returns `conflict`. That refusal has no effect, so its request ID is released. While a departure or a `space_redeem` waits on the other Endpoint, it holds its own request ID but not the local mutation lock, so other mutations are still answered.

`ui_init` atomically establishes or replaces the Web password and revokes existing sessions. The public CLI uses this operation for both first setup and recovery. Lower-level `ui_password_set` and `ui_password_reset` operations retain their explicit preconditions for local API callers.

`ui_start` carries `host` (an IP address or hostname) and `port` (0–65535; zero selects a free port). `ui_stop` and `ui_status` carry no fields. All three return `ui_status` with `running` and nullable `url`. These are private IPC lifecycle instructions, have no `request_id`, execute on every call, and never participate in durable replay. Each successful `ui_start` restarts an existing service. A new daemon always starts with WebUI stopped. HTTP routes do not expose UI initialization or lifecycle control.

### Service grants, exposure, and operation status (Phase 2)

`space_member` carries the signed service grants `files_read`, `files_write`, `exec`, `shell`,
`proxy`, (since Phase 3) `process`, (since Phase 4) `agent`, and (since Phase 5) `service` beside
`echo` and `relay_provider`. These are what a member may ask other members for. They
never make any Runtime serve anything by themselves.

- **`space_grant`** carries a request ID, `space_id`, `peer_endpoint_id`, and all eight grant
  booleans. It replaces exactly those eight bits of the member and returns `space_granted` with the
  signed `space_member`. Only the Space owner can sign it; any other Runtime gets `unauthorized`,
  and an unknown Space or member gets `not_found`. Neither refusal has any effect. An unchanged
  grant succeeds without signing a new generation. Grant changes reach peers through ordinary
  control sync.
- **`service_expose`** carries a request ID, `space_id`, and one exposure in flat nullable form:
  - `kind` is `files_root`, `exec`, `shell`, `proxy`, `process`, `agent`, or `service`;
  - `root`, `path`, and `access` (`read_only` or `read_write`) are required exactly for
    `files_root`;
  - `host` (`*` or a canonical host) and the inclusive `port_start`–`port_end` range are required
    exactly for `proxy`;
  - `agent` (an `AgentId`) is required exactly for `agent`, and is consent for that Space's
    members to use that one Agent;
  - `service` (a hosted service name) is required exactly for `service`, and is consent for that
    Space's members to call that one hosted service;
  - every inapplicable field is `null`.

  The result is `service_exposed`. A root path must be an existing absolute directory; it is stored
  canonicalized. A root name is bound Runtime-wide to one directory, so re-binding it elsewhere
  returns `conflict`. Exposing the same selector again updates it in place. A Runtime holds at most
  256 exposures, and refuses one with `invalid_input` and no effect when the resulting
  `service_exposure_list` answer would no longer fit one 65,536-byte response.
- **`service_unexpose`** names an exposure by the same selector without `path` and `access`. It
  returns `service_unexposed` with the removed exposure, or `not_found` without effect.
- **`service_exposure_list`** returns `service_exposures`, an array of `service_exposure`. A
  database written before that size check may hold exposures that no longer fit one response;
  the list then answers `conflict` with a remediation, and each exposure can still be removed
  with `service_unexpose` until the list fits again.
- **`operations_status`** returns `operations`: `active_count` and at most 64 `active` operations
  with `elapsed_ms`, plus `recent_count` and at most 64 of the most recent completed operations,
  newest first, with `result`, nullable `error`, byte counts, and `duration_ms`. Operations carry a
  request ID, the peer, the service, a content-free operation name, and a direction. They never
  carry paths, argv, environment, or content. This state is in memory only.

These commands are local IPC operations. The HTTP layer routes only five reads, all for the
Web console's Services view. Each needs an authenticated session:

- `GET /api/v1/services` returns the `service_exposures` payload.
- `GET /api/v1/operations` returns the `operations` payload.
- `GET /api/v1/processes` returns the first `processes` page of `process_list`.
- `GET /api/v1/agents` returns `{agents, more_agents, tasks, more_tasks}`: the first page of
  `agent_list` and of `agent_task_list`. Each Agent keeps only `agent`, `description`, `tools`,
  `max_tasks`, `enabled`, `generation`, `live_tasks`, and `instance`, never its program,
  arguments, working directory, or environment names. Tasks are the content-free summaries.
- `GET /api/v1/hosted-services` returns the first `hosted_services` page of `service_hosted_list`.

Grants and consent
(`space_grant`, `service_expose`, `service_unexpose`) are never routed over HTTP, so a browser
session cannot make this Runtime serve anything.

`stream_open` carries a request ID, `target_endpoint_id`, and `service` (`files`, `transfer`,
`exec`, `shell`, `proxy`, `process`, `agent`, or `service`). It upgrades one IPC connection to the stream plane:

1. After the `stream_opened` response, the connection carries Services v1 frames
   (`docs/protocol/services-v1.md`) verbatim in both directions and no further local API frames.
2. The client sends exactly one `open` frame. Its request ID, target, and service must equal the
   upgrade's; otherwise it receives `reject(invalid_input)`.
3. The Runtime dials the target on the family ALPN and relays frames with one-frame buffers.
   Dialing and writing `open` share 15 seconds. A target that grants no stream in that time is
   refused with `reject(timed_out)` and the request ID is released; one that takes no `open` in
   that time, or no later frame within 30 seconds, is treated as lost.

The client keeps its side open until it receives a terminal frame. Closing the connection earlier
cancels the operation. A client that takes no frame for 30 seconds is treated as gone and the
operation is cancelled; the `accept` and the terminal frame are given 5 seconds.

Admission and replay:

- **Admission.** Stream connections use their own pool of 64. They never hold a business connection
  slot or the mutation lock while streaming. A self-targeted upgrade is `invalid_input`.
- **Replay scope.** Side-effecting operations (Files mkdir, write, rename, and remove; upload;
  Exec; Shell; Proxy) reserve their request ID in the same durable replay table as local API
  mutations. The fingerprint covers the service, the target, and the exact `open` payload. Reads
  never reserve.
- **Not started.** A target `reject` or a failed dial releases the reservation.
- **Completed.** A target `outcome` is recorded. The same request later receives that exact
  frame again without contacting the target. A terminal the Runtime has read is recorded even
  when a client that cancelled and stopped reading never takes it within the cancel grace.
- **Indeterminate.** A stream lost after `open` was written is recorded as a failed
  `outcome(outcome_unknown)`, which is also the answer for a reservation left by a previous
  Runtime process.
- **Duplicates.** A request ID reused with a different `open` receives `reject(conflict)`. One
  still running receives `reject(unavailable)`.

Process mutations (`start`, `stdin`, `attach` with stdin, `signal`, `terminate`, `delete`) follow
these same rules. A `start` also binds its request ID on the target, so a retry that reaches the
target by any path answers the existing Process instead of creating a second one
(`docs/protocol/process-v1.md`).

### Hosted Processes (Phase 3)

These four commands are local IPC operations for the operator of the Runtime that hosts the
Processes. Only the first `process_list` page is routed over HTTP, read-only, as
`GET /api/v1/processes`. Inspection, termination, and deletion are not. Starting a Process is
always a remote `start` over the
stream plane, because a Process belongs to the Endpoint that created it.

- **`process_list`** carries a nullable `after` Process ID and returns `processes`: at most 64
  `process_summary` values in Process ID order, archived tombstones included, and a nullable
  `next` to continue from.
- **`process_inspect`** carries a `process_id` and returns `process`:
  - the summary;
  - `start` (program, args, cwd, `env_names`, stdin policy, `log_limit`, labels), or `null` once
    archived;
  - the retained output window `log_first_available`–`log_end`.

  Environment values are never shown, only names. Arguments above 32 KiB of encoded start are
  replaced by `[]` with `args_elided: true`.
- **`process_terminate`** carries a request ID, `process_id`, and `grace_ms` (0–60000). It sends
  a terminate, waits the grace period, then kills the Process group. It answers
  `process_terminated` with the resulting summary once the end is recorded. An already terminal
  Process answers its current summary.
- **`process_delete`** carries a request ID and `process_id`. It archives a terminal Process,
  removes its output and start, and answers `process_deleted` with the tombstone.

A summary carries `process_id`, `creator_endpoint_id`, `request_id`, nullable `name`, `state`
(`created`, `starting`, `running`, `exited`, `terminated`, `failed`, or `lost`), nullable
`reason`, nullable `exit_code` and `signal`, `created_at_ms`, nullable `started_at_ms` and
`ended_at_ms`, and `archived`.

Errors:

| Condition | Error |
| --- | --- |
| No Process with that ID | `not_found`, no effect |
| `process_delete` on a Process that has not ended | `conflict` with remediation, no effect |
| `process_terminate` on a Process that never started | `conflict` with remediation, no effect |
| This Runtime does not host Processes | `unavailable`, no effect |
| Supervisor not answering conclusively | `unavailable`; the Runtime keeps following the Process |

The two mutations use ordinary mutation replay. `process_terminate` holds the local mutation lock
while it waits for the supervisor: at most the grace period plus 10 s, so other local mutations
wait up to 70 s behind it. The IPC client waits for a reply without a deadline.

### Hosted Agents (Phase 4)

These commands are local IPC operations for the operator of the Runtime that hosts the Agents
(`docs/protocol/agent-v1.md`). The operator sees every task of every Agent. Tasks it submits are
created as this Endpoint. Only the read-only `GET /api/v1/agents` above is routed over HTTP.
Another Endpoint's Agents are used through `stream_open` with service `agent`.

Definitions and instances:

- **`agent_define`** carries a request ID and:
  - `agent`, an `AgentId`;
  - `description`, at most 256 bytes;
  - `program`, `args` (at most 256), and a nullable absolute `cwd`;
  - `env`, at most 64 `{name, value}` pairs;
  - `tools`, a set of the families `files`, `transfer`, `exec`, `process`, `proxy`, and `agent`;
  - `max_tasks`, 1–16.

  It returns `agent_defined` with the Agent and `created`. A new Agent starts stopped. Redefining
  an existing Agent replaces its definition, and needs it stopped.
- **`agent_undefine`** removes a stopped Agent's definition and returns `agent_undefined`.
  Every exposure naming it is withdrawn in the same commit, so an Agent defined later under that
  name needs fresh consent; replacing a definition keeps them.
- **`agent_start`** enables an Agent and starts its first instance at once. **`agent_stop`**
  disables it: its unfinished tasks fail with `agent_stopped`, and its instance is terminated. Both
  return the Agent (`agent_started` and `agent_stopped`).
- **`agent_list`** carries a nullable `after` `AgentId`. It returns `agents`: at most 16 Agents in
  name order, and a nullable `next`. An Agent view carries:
  - the definition, with environment names only and never values;
  - `elided: true`, with `args` and `env_names` emptied, when those two exceed 2 KiB encoded;
  - `enabled`, `generation`, `defined_at_ms`, `changed_at_ms`;
  - `live_tasks`, the number of unfinished tasks;
  - the nullable current `instance`: `generation`, nullable `process_id`, `ready`,
    `started_at_ms`, and nullable `ended_at_ms`.

Tasks:

- **`agent_task_list`** carries a nullable `agent` and a nullable `after` task ID. It returns
  `agent_tasks`: at most 32 task summaries in task ID order, and a nullable `next`.
- **`agent_task_inspect`** carries a `task_id` and returns `agent_task` with these fields:
  - `task`, the summary;
  - `input` and `result`;
  - `error` and the newest `progress`;
  - `artifacts`.

  A message above 20 KiB encoded is replaced by `null`, and its `input_elided` or
  `result_elided` flag is set.
- **`agent_task_events`** carries a `task_id` and `after` (a sequence number). It returns
  `agent_task_events`: at most 64 retained events after `after`, oldest first, and `more`. A
  message larger than 20 KiB encoded is elided and flagged in the same way.
- **`agent_task_artifact_read`** carries a `task_id`, an artifact `name`, and an `offset`. It
  returns `agent_task_artifact`, which carries:
  - `artifact`, the reference: name, media type, size, and BLAKE3;
  - `offset`;
  - `content`, at most 32 KiB from `offset` as standard base64;
  - `eof`.

  A client reads chunk by chunk and verifies the whole content against the BLAKE3. Blobs are
  immutable, so the chunks of one artifact never mix two contents.
- **`agent_task_submit`** carries a request ID, `agent`, and a `message`
  (`{role, parts: [{kind: text|data, content}]}`). It returns `agent_task_submitted` with the
  summary and `existing`. The request ID binds exactly one task: a retry with the same body
  answers that task with `existing: true`, and a different body is `conflict`.
- **`agent_task_send`** carries a request ID, `task_id`, and a `message`. It records the message
  as an event and delivers it to the task's instance; a queued task receives it right after the
  task itself, on assignment. It returns `agent_task_sent`. The task may be
  queued, running, or waiting; an Agent asks for input by moving the task to `waiting`.
- **`agent_task_cancel`** carries a request ID and `task_id`. It returns `agent_task_cancelled`
  with the summary at once:
  - a queued task is already cancelled;
  - a running or waiting task has its cancellation recorded, and ends once the Agent stops it,
    or after the 10 s grace period.

  Neither the command nor the local `ma2a agent cancel` waits for the end; `ma2a agent watch TASK`
  follows the task until it ends.

A task summary carries:

- `task_id`, `agent`, `creator_endpoint_id`, and a nullable `from_agent`;
- `request_id`;
- `state` (`queued`, `running`, `waiting`, `completed`, `failed`, or `cancelled`) and a nullable
  `reason`;
- `created_at_ms`, and nullable `started_at_ms` and `ended_at_ms`;
- `last_seq`.

`from_agent` is attribution only.

Errors:

| Condition | Error |
| --- | --- |
| No such Agent, task, or artifact | `not_found`, no effect |
| `agent_define` or `agent_undefine` on a started Agent | `conflict` ("stop it first"), no effect |
| `agent_task_submit` to a stopped Agent | `conflict` ("start it first"), no effect |
| `agent_task_send` to a task that has ended or is being cancelled | `conflict` ("the task has already ended"), no effect |
| A reused request ID with a different body | `conflict`, no effect |
| A task bound is reached | `unavailable` ("retry once tasks end"), no effect |
| This Runtime does not host Agents | `unavailable`, no effect |

### Hosted services (Phase 5)

These commands let the operator of a Runtime host third-party services (`docs/protocol/service-v1.md`).
Only the first `service_hosted_list` page is routed over HTTP, read-only, as
`GET /api/v1/hosted-services`; registration, lifecycle, and process upgrades are local IPC
operations only. Another Endpoint's services are discovered and called through
`stream_open` with service `service`.

Registrations:

- **`service_register`** carries a request ID, `service` (a `ServiceName`), and `manifest`:
  - `description`, at most 256 bytes;
  - `max_calls`, 1–64: the bound on running calls;
  - `versions`, 1–8 in strictly ascending order, each `{version, operations}` with 1–16 distinct
    `{name, interaction}` operations (`unary`, `stream`, `bytes`, or `resource`), and at most 64
    operations in all.

  It returns `service_registered` with the service and `created`. A new service starts stopped;
  replacing a registration needs the service stopped. At most 64 services are registered.
- **`service_unregister`** removes a stopped service's registration and returns
  `service_unregistered`. Every exposure naming it is withdrawn in the same commit, so a service
  registered later under that name needs fresh consent; replacing a registration keeps them.
- **`service_start`** enables a service and **`service_stop`** disables it; both return the
  service (`service_started` and `service_stopped`). A stop tells the attached process to stop,
  withdraws calls it has not accepted, and cancels running ones.
- **`service_hosted_list`** carries a nullable `after` and returns `hosted_services`: at most 16
  services in name order, and a nullable `next`. A hosted service view carries:
  - `service`, the `manifest`, and `enabled`;
  - `state`: `stopped`, `unavailable` (started without a ready process), or `ready`;
  - `running_calls`, and `calls`, the calls per version since this Runtime started or the service was last registered;
  - `created_at_ms` and `updated_at_ms`.

Process upgrades (no request ID; never replayed):

- **`service_attach`** carries `service` and turns the connection into the service's link,
  answered by `service_attached` with the registered manifest. A service has at most one link.
  After the answer each frame carries one JSON message of at most 1 KiB:
  - the process sends `{"t":"ready"}` or `{"t":"unavailable"}`; anything else ends the link;
  - the Runtime sends `{"t":"call","call":"<32 hex>"}` for each call waiting to be accepted, and
    `{"t":"stop"}` once before it ends the link.

  The link's end makes the service `unavailable` at once.
- **`service_accept`** carries `service` and `call_id` and turns a fresh connection into that one
  call. Its answer `service_call` carries `service`, `call_id`, `caller_endpoint_id`,
  `request_id`, `version`, `operation`, and `body` (standard base64, at most 8 KiB decoded). The
  connection then carries Services v1 frames for the call. A call not accepted within 10 s is
  withdrawn. The daemon keeps connection slots for `service_accept` apart from the ones other
  local requests share, so an accept is admitted even when every one of those is taken; it needs
  no `handshake` first, and its own `version` is checked.

Errors:

| Condition | Error |
| --- | --- |
| No such service, or no such waiting call | `not_found`, no effect |
| `service_register` or `service_unregister` on a started service | `conflict` ("stop it first"), no effect |
| `service_attach` while stopped | `unavailable` ("start it first") |
| `service_attach` while another process is attached | `conflict` |
| A 65th registration | `unavailable` ("unregister one first"), no effect |
| This Runtime does not host services | `unavailable`, no effect |

## Mutation Replay

Every durable mutation carries a client-generated `request_id`. The Runtime fingerprints the canonical typed command after version and input validation:

- New identifier and payload: execute once.
- Same identifier and same payload: return the prior result without executing again.
- Same identifier and different payload: return `conflict` before mutation or revision read.

The Runtime durably reserves a new identifier before execution. Execution errors carry effect knowledge: definitely not started, committed at a known revision, or indeterminate. Only a positively established no-effect failure releases its reservation. Committed and indeterminate failures retain an exact terminal error, including a warning to inspect state rather than assume rollback. The same request replays that error without execution; changing its payload conflicts. If completion persistence fails or a process crashes, the reservation remains Pending and returns `unavailable`. Raw invitation secrets are never retained in SQLite to reconstruct a result.

Terminal response caching is bounded to 1,024 entries, 8 MiB of encoded responses, and 65,536 bytes per response. FIFO eviction removes cached responses but preserves permanent RequestId/fingerprint fences. An evicted response is unavailable; its request never becomes executable again. Pending reservations are not evicted while their Runtime runs, so admission exhaustion fails closed. A reservation left by a stopped or crashed Runtime can never be settled; when the next Runtime starts, before it admits anything, it frees those slots and keeps their fences, so each such request is still answered as pending (`unavailable` locally, `outcome_unknown` on a stream) and never becomes executable again. Schema 6 seeds fences from all existing replay rows; identifiers already evicted by older versions cannot be recovered. Fences contain no command bodies and grow with admitted identifiers. A replayed shutdown response does not send a new shutdown signal.

## Snapshots And Events

`snapshot_fetch` and authenticated `GET /api/v1/snapshot` return the same authoritative `RuntimeSnapshot` projection. Snapshot Spaces use the `snapshot_space` model, which carries the signed chain head the Runtime already holds: latest `generation`, `chain_hash`, `member_count`, and `revoked_count`. The global snapshot carries no member arrays. Command results such as `space_created` keep the smaller `space` model, because ordinary mutation results need only that committed Space summary. Each connection carries the bounded `observations` buffer Iroh retains for that peer, at most eight entries, each with its observed path, round-trip estimate, and a closed `error_class` whose `none` member means no failure was recorded. Relay projections keep the two relay ontologies apart. `private_relay_candidates` describes MA2A Private Relays, each a role hosted by an MA2A Endpoint, and carries `provider_endpoint_id`, `relay_url`, `covered_space_ids` and the `home_compatible` verdict, so the verdict and the coverage that produced it travel together. `public_relay_fallbacks` describes external public Iroh relays, which are not MA2A Endpoints, are never Space members, and therefore carry only `relay_url`, `enabled` and `observed_connected`; no provider identity or Space coverage is synthesised for them. `reachability.state` is the Runtime's own `RelayReachability` value projected verbatim, and is the only authoritative source for it; `observed_relay_state` keeps its two facts apart by name: `private_relay_provider_running` reports whether this Runtime currently hosts its embedded Private Relay service, and `public_relay_connected` reports whether Iroh currently sees this Endpoint connected through an enabled Public Relay Fallback. Neither may be used to infer `reachability.state`. `control_rounds` retains at most sixteen completed control rounds in memory only; it is a diagnostic, is never persisted, and is empty after a restart. A round is `empty` when it had no peer to dial, and `failed` when it dialed peers but none of them answered every request, even if part of what they sent was committed; an explicit synchronization waiting on such a round fails as well. The HTTP route returns the snapshot payload directly. Runtime/SQLite supplies the revision, verified Space membership and member counts, and password state from one read transaction. Fields whose current source is actor-local or time-derived remain conservative constants until they are persisted under that same revision. A Space `name` is the shared name signed into that Space's genesis body, so every member projects the same value; a Space created before genesis carried a name has no persisted name and falls back to its canonical lowercase Space ID.

`space_details_fetch` takes a `space_id` and returns `space_details`: a `revision`, a `space` summary, and the complete sorted `members` set with labels and capability grants (at most 64). These fields come from one SQLite read transaction for that Space. A missing Space or one whose signed membership no longer includes the local Endpoint returns `not_found`. Authenticated `GET /api/v1/spaces/{space_id}` forwards this query through IPC and returns its payload; it maps `not_found` to HTTP 404. It does not build a global snapshot.

The frontend caches details by Space ID and chain hash, loads at most four details concurrently, and distinguishes pending or failed reads from an empty membership set. Spaces, Peers and Map share this cache. Changed heads and removed Spaces invalidate cached details; disconnect/resync clears the cache and invalidates pending responses. Unrelated revisions retain matching details. A detail whose head differs from the current summary cannot be installed.

`snapshot_stamp` returns only the durable `revision` and this process's `runtime_boot_id`. SSE uses this lightweight IPC query for baseline and polling, without constructing a full snapshot. Its revision comes from the Store so independent authentication mutations remain observable. The success envelope revision equals the payload revision for snapshots, details and stamps.

This exact v1 schema update requires coordinated Runtime and frontend upgrades.
The 65,536-byte **frame** bound remains unchanged. A small snapshot is one ordinary
JSON response. A larger snapshot (or snapshot-derived `space_list`) is frozen once, then transferred as ordered
`snapshot_fragment` objects with `revision`, `runtime_boot_id`, zero-based `index`,
`last`, and `data_hex`. Each fragment carries at most 16,384 original bytes as
lowercase hex (32,768 ASCII bytes), plus less than 232 bytes of fixed metadata even
at maximum-width revision/index. No wire frame exceeds 33,000 bytes. Every
non-final fragment is full. IPC fragments use the original correlation and contain
the complete stamped response (including its `runtime_boot_id` extension); HTTP uses newline-delimited JSON
(`application/x-ndjson`) and fragments the snapshot payload. Small HTTP responses
remain `application/json`.

All fragments belong to the same frozen projection, never repeated live reads.
Clients require contiguous indexes, identical revision/boot, complete final data,
and a matching reconstructed payload revision before installing the snapshot.
An interrupted stream is discarded; a new request takes a fresh snapshot. There
is no server-side cursor cache or process-restart continuation. Collection size
limits are not invented as capacity restrictions on otherwise legal Spaces or
relay coverage. Per-entity field limits, 64-member details, eight connection
observations and sixteen retained control rounds remain unchanged. Snapshot
assembly uses memory proportional to the complete state, while transport frames
and writes remain bounded. This is lossless transfer, not summary truncation.


Authenticated `GET /api/v1/events?since=<revision>` accepts only a baseline equal to the current authoritative revision. A stale baseline receives one `resync-required` SSE event and the stream closes. Accepted streams use a bounded per-client queue and event IDs equal to revision. A later consecutive revision emits a typed `snapshot_invalidated` event; a skipped revision, queue overflow, Runtime unavailability, or Runtime restart emits `resync-required` when possible and closes. An expired or revoked session closes immediately without a revision or state-bearing frame. Polling and session-validity checks do not extend session deadlines or advance revision. Sliding an active session does not advance the revision either: `last_seen_at_ms` and `idle_expires_at_ms` reach no snapshot projection, so a slide is not observable state. Advancing for it would make every authenticated read a state change and let the console's own reads drive an endless invalidate-and-refetch cycle. Session creation, deletion and revocation still advance, because they change the `active_sessions` count the snapshot carries.

Authenticated same-origin mutation routes require the session cookie, matching `X-CSRF-Token`, and exact `application/json`. Each route accepts one existing local API command operation and forwards the parsed typed `Command` through `LocalApiClient`; Web handlers do not mutate Runtime or SQLite state directly.

SSE events are best-effort projections, not a durable event log or replay mechanism. A disconnect, duplicate, reordered event, revision gap, `snapshot_invalidated`, or `resync-required` signal makes client state uncertain. The frontend discards further incremental events and replaces the entire state with the next successful snapshot rather than merging it with uncertain state. Losing a stream that opened reloads the snapshot at once; a stream that fails before it opens, as when every event connection is taken, is retried after 250 ms, 1 s, and then every 4 s, so a refused stream never turns into a reload loop.
