# ma2a-phase-4 — Agent Runtime / A2A

Baseline: Phase 3 freeze `59258db` (Persistent Process Layer, frozen infrastructure).

## Goal

An **Agent** is a logical workload that an Endpoint hosts, next to its Processes:

- An Agent is never a cryptographic identity.
- Remote authentication stays Endpoint ↔ Endpoint.
- Authorization still needs one complete Space path.
- An `AgentId` only attributes and routes: caller Endpoint → target Endpoint → target `AgentId`.

Phase 4 delivers:

- the Agent model;
- an Agent runtime built on Phase 3 Processes, behind a typed local adapter;
- durable Agent tasks;
- the native Agent service;
- mediated tool access to the Phase 2 and 3 services;
- content-addressed artifacts;
- an external A2A v1.0 adapter.

There is no delegation, no multi-agent scheduler, and no LLM vendor model in Core.

## Architecture decisions

### A1 — Model

| Type | Definition |
| --- | --- |
| `AgentId` | Operator-chosen name, unique per Endpoint. 1–63 bytes of `[a-z0-9-]`, starting with a letter, with no trailing `-`. Shown as is. |
| `AgentDefinition` | Local operator configuration (see below). Changing it requires the Agent to be stopped. |
| `AgentInstance` | One running incarnation of a definition: a Phase 3 Process created by the local Endpoint, with a generation number. At most one per Agent. |
| `AgentTaskId` | 16 random bytes, shown as 32 lowercase hex characters. |
| `AgentTaskState` | `queued`, `running`, `waiting`, `completed`, `failed`, `cancelled`. |

An `AgentDefinition` holds:

- a description (≤ 256 bytes);
- the program, args, cwd and env, bounded like a `ProcessSpec`;
- the allowed tool families;
- `max_tasks`, the concurrent tasks per instance: 1–16, default 4.

### A2 — Authority: one more signed bit, one more consent kind, creator ownership

- Member capability bit 8 (value 256) is `agent`. It is signed by the Space authority like every earlier bit and defaults off. An older Runtime decodes the new bit as unknown and refuses the grant, the same caveat as Phase 3.
- Per-Space exposure kind `agent` (kind 5) carries one `AgentId` as its selector: `ma2a service expose agent SPACE AGENT`. It is consent for that Space's members to use that one Agent.
- `ServiceAccess::Agent(AgentId)` requires one complete Space in which:
  - both Endpoints are members;
  - the caller holds `agent`;
  - this Runtime exposes that exact Agent.

  The coarse admission `ServiceAccess::AgentAdmission` requires `agent` plus at least one exposed Agent in the same Space. Grants and exposures never combine across Spaces, and the Space is never disclosed.
- Every task operation names its `AgentId`, and the host checks that the task belongs to that Agent. An `AgentId` that is undefined, not exposed to the caller, or not owned by the caller's path all answer `unknown_agent`, identically.
- **Ownership.** A remote caller sees only the tasks its own Endpoint created. Any other task is `unknown_task`. The local operator sees every task.
- **Attribution only.** A submitting Agent's `AgentId` (`from_agent`) is recorded and shown, never used to authorize. The authority of an Agent's tool calls is that of its hosting Endpoint.
- **Withdrawal converges.** After each durable change and every 15 s, every non-terminal task whose remote creator no longer has `Agent(agent)` is cancelled with reason `unauthorized`.

### A3 — Transport

- ALPN `ma2a/agent/1` (`ServiceKind::Agent`, wire code 8). It is negotiated like the other service ALPNs: only while the Runtime has at least one Space.
- One operation per stream, using Services v1 frames. The `open` body is tagged CBOR.

| Operation | Class | Frames after `accept` |
| --- | --- | --- |
| `agents {after?}` | read | outcome: a page of Agent cards this caller may use |
| `submit {agent, message, from_agent?}` | mutation | `accept {task, existing}`, outcome: summary |
| `send {agent, task, message}` | mutation | outcome: summary |
| `get {agent, task}` | read | outcome: details (result, artifacts, error) |
| `tasks {agent, after?}` | read | outcome: a page of the caller's tasks |
| `watch {agent, task, after}` | read | `accept {state, next}`, then one `data` (ch 1) per event, then an outcome with the summary once the task is terminal and every event has been delivered |
| `cancel {agent, task}` | mutation | outcome: summary after the task ended |
| `artifact {agent, task, name, offset}` | read | Transfer v1 download frames: accept with content identity, data ch 1, outcome |

Mutations use the Phase 1 durable replay through the unchanged stream-plane rules. Reads never reserve.

### A4 — RequestId binds exactly one task

- The target's Store binds `(creator, RequestId)` to one `AgentTaskId` with a unique key and keeps a fingerprint of `(agent, canonical submit body)`.
  - A retry with the same fingerprint answers the existing task (`existing = true`) in whatever state it is.
  - A different body answers `reject(conflict)`.
- The binding is inserted in the same transaction as the task row, before any delivery to the Agent.
- A tool call's RequestId is derived:

  ```text
  RequestId = blake3::derive_key("ma2a agent tool call v1", task ‖ call)[..16]
  ```

  So re-executing a tool call after a daemon restart replays it through the caller's stream-plane replay table and never acts twice.

### A5 — Task lifecycle (durable)

```text
queued ──► running ◄──► waiting
  │           │            │
  └───────────┴────────────┴──► completed | failed | cancelled
```

| State | Meaning |
| --- | --- |
| `queued` | Recorded; not yet assigned to an instance. |
| `running` | Assigned to an instance generation and delivered (at least once) to it. |
| `waiting` | The Agent asked for input (A2A `input-required`); `send` delivers it. |
| `completed` | The Agent reported a result: a small inline message and/or artifacts. |
| `failed` | Reason: `agent_failed` (the Agent reported it), `agent_lost` (its instance ended), `agent_stopped` (the operator stopped or undefined the Agent), or `limit`. |
| `cancelled` | Reason `requested` (a `cancel`) or `unauthorized` (withdrawal). |

- **Assignment.** A queued task is assigned only to a `ready` instance with fewer than `max_tasks` active tasks, in FIFO order. Moving it to `running` and adding the delivery to the outbox happen in one transaction.
- **Cancellation.**
  - `queued`: cancelled at once.
  - `running` or `waiting`: `cancel_requested` is recorded and `cancel` is delivered. The task ends as soon as the Agent reports a terminal state, or after a 10 s grace period as `cancelled(requested)`, whichever comes first. Later Agent output for that task is ignored.
  - The operation answers once the task is terminal.
- **Agent crash.** When the instance Process ends while the Agent is enabled:
  - its `running` and `waiting` tasks fail with `agent_lost`;
  - queued tasks stay queued;
  - a new generation starts after a backoff of 1 s, 2 s, 4 s … 60 s, reset after 60 s of uptime.
- **Runtime restart.** Supervisors outlive the daemon, so the instance Process is re-adopted by Phase 3. The link resumes from its durable cursor, re-sends unacknowledged deliveries, and re-executes pending tool calls. The tasks simply continue.
- **Retention.**
  - A terminal task, with its events and artifact references, is deleted 7 days after it ended, together with its binding.
  - Artifact blobs that no task references are then garbage-collected.
- **Limits.**
  - Non-terminal tasks: 16 per creator, 256 per Runtime.
  - Retained terminal tasks: 64 per creator, 1,024 per Runtime.
  - Over a limit, `reject(concurrency_exceeded)` with no effect.

### A6 — Messages, events, artifacts

- **Message.**
  - Contents: `{role: user|agent, parts: [text(String) | data(JSON text)]}`.
  - At most 16 parts and 16 KiB encoded. The target validates `data` parts as JSON.
  - Larger payloads must be artifacts.
- **Event log** per task, durable, with `seq` starting at 1. Event kinds:
  - `state {state, reason?, message?}`
  - `message {message}`, from the user or the agent
  - `progress {text ≤ 1 KiB}`
  - `artifact {ref}`

  Bounds:
  - `state`, `message` and `artifact` events are never pruned. A task holds at most 256 of them; the next one fails the task with `limit`.
  - Only the newest 64 `progress` events are retained. A watcher that reconnects with `after = seq` receives every retained event after `seq`. Gaps appear only in progress.
- **Artifact.**
  - A ref is `{name (1–128 safe bytes), media_type (≤ 127), size, blake3}`.
  - The Agent publishes a file from its workspace with the `artifact.publish` tool. The daemon hashes and copies it into the content-addressed store `<state>/agents-v1/blobs/<blake3 hex>`, so identical content is stored once.
  - Limits: 256 MiB per artifact, 16 per task.
  - Retrieval is the `artifact` operation, served by the unchanged Transfer v1 download path: BLAKE3-verified, resumable by offset, with `content_changed` impossible because blobs are immutable.

### A7 — Agent runtime and the typed adapter (JSON lines over stdio)

- **The instance is a Phase 3 Process.**
  - It is created by the local Endpoint through an internal start, with stdin set to a pipe and a 16 MiB log limit.
  - Its cwd is `<state>/agents-v1/<agent>/workspace`, unless the definition gives one.
  - Its env adds `MA2A_AGENT_ID`, `MA2A_AGENT_WORKSPACE` and `MA2A_AGENT_PROTOCOL=1`.
  - The instance start's `RequestId` is derived from `(agent, generation)`, so a crash between the Process commit and the instance commit never starts two instances.
  - Locally created Processes are exempt from withdrawal convergence and invisible to remote callers, because no remote peer can be the local Endpoint.
- **Agent → Runtime.**
  - The Agent writes one JSON object per line to stdout, at most 64 KiB per line. Stderr is free-form log.
  - The daemon follows the Process log through an authenticated supervisor `attach` from its durable cursor and holds the stdin writer lease.
  - Each complete line is applied in one Store transaction, together with the advance of the cursor to just past its newline. Every Agent message therefore takes effect exactly once, across daemon crashes.
- **Runtime → Agent.**
  - Each delivery gets an instance-scoped `seq` and is first stored in the instance outbox, then written to stdin as one line in one data frame.
  - The Agent acknowledges with `ack {seq}`. On every reconnect the daemon re-sends all unacknowledged deliveries in order.
  - The Agent ignores any `seq` it has already processed, so delivery is at least once and the effect exactly once.
- **Messages.** Tagged with `"t"`.
  - Agent → Runtime:
    - `ready {protocol: 1}`, which must arrive within 30 s, or the instance counts as crashed;
    - `ack {seq}`;
    - `state {task, state: running|waiting|completed|failed|cancelled, message?, error?}`;
    - `message {task, message}`;
    - `progress {task, text}`;
    - `tool {task, call, name, args}`.
  - Runtime → Agent:
    - `task {seq, task, from: {endpoint, agent?}, message}`;
    - `message {seq, task, message}`;
    - `cancel {seq, task}`;
    - `tool_result {seq, task, call, ok | error: {code, message}}`.
  - Output that violates the protocol, or names a task this instance does not hold, is counted and ignored. It never fails another task.
- **Trust.** The local adapter is not a security boundary against the Agent program. The Agent runs as the daemon's OS user, and Phase 1 treats same-user processes as the local operator. The tool policy is a guardrail plus attribution, not isolation. This is documented.

### A8 — Tool access (no delegation)

An Agent asks for a tool with `tool {task, call, name, args}`. The daemon:

1. checks the definition's tool families;
2. records the call durably (`pending`);
3. runs it as a same-user stream-plane client of its own Runtime, so authority, Endpoint identity, replay and audit are exactly those of the CLI;
4. records the result and delivers `tool_result`.

Pending calls of live tasks are re-executed after a restart with the same derived RequestId. At most 8 calls run at once per task, and at most 256 per task in total.

| Family | Tools |
| --- | --- |
| `files` | `files.stat {endpoint, root, path}`, `files.list {endpoint, root, path}` |
| `transfer` | `transfer.get {endpoint, root, path, to}`, `transfer.put {endpoint, root, path, from, replace}` |
| `exec` | `exec.run {endpoint, program, args, cwd?, env?}`: exit status, plus stdout and stderr as lossy UTF-8, 1 MiB each |
| `process` | `process.start {endpoint, program, args, cwd?, env?, name?}`, `process.inspect {endpoint, id}`, `process.terminate {endpoint, id, grace_ms?}` |
| `proxy` | `proxy.open {endpoint, host, port}` → `{listen}`: a loopback listener that lives as long as the task, with one proxy stream per accepted connection |
| `agent` | `agent.submit {endpoint, agent, message}`, `agent.get`, `agent.wait {…, timeout_ms ≤ 300 s}`, `agent.send`, `agent.cancel`, `agent.artifact {endpoint, agent, task, name, to}` |
| (always) | `artifact.publish {task-local name, path, media_type?}` |

- Workspace paths (`to`, `from`, `path`) are relative and must stay inside the instance workspace. The check is canonical, and no symlink may escape.
- Shell is not offered. It is an interactive terminal; an Agent that needs a command uses `exec.run` or `process.*`. This is documented.
- Tools target remote Endpoints only. The Agent is a local process and already has local OS access.
- Tool errors use the Services v1 status names, plus `tool_denied`, `invalid_tool` and `outcome_unknown`.

### A9 — External A2A v1.0 adapter

`ma2a a2a serve [--listen 127.0.0.1:PORT]` is a separate adapter process, a same-user local API client. It speaks the A2A v1.0 JSON-RPC 2.0 binding over HTTP.

- **Methods.**
  - Supported: `SendMessage`, `SendStreamingMessage` (SSE), `GetTask`, `ListTasks`, `CancelTask`, `SubscribeToTask` (SSE).
  - Unsupported: push-notification methods and `GetExtendedAgentCard`, which answer the A2A errors.
- **Routes.**
  - Local Agents: `/agents/{agent}`.
  - Remote Agents: `/endpoints/{endpoint}/agents/{agent}`, routed through the stream plane under this Endpoint's authority.
  - Each route has an Agent Card at `…/.well-known/agent-card.json`.
- **Authentication.** HTTP bearer. The token is random per run and written to a 0600 file in the state directory, never logged. Loopback only by default.
- **Mapping.**
  - Idempotency: the A2A `messageId` becomes the RequestId, as `blake3(messageId)[..16]` with a domain separation.
  - States: `queued` → `TASK_STATE_SUBMITTED`, `running` → `WORKING`, `waiting` → `INPUT_REQUIRED`, `completed` → `COMPLETED`, `failed` → `FAILED`, `cancelled` → `CANCELED`.
  - Parts: text ↔ text, data ↔ data.
  - Artifacts become A2A artifacts with a file part whose URL is served by the adapter.
- **Mismatches,** documented and not emulated:
  - A2A `contextId` grouping across tasks is not supported (the context is the task).
  - Push notifications.
  - File parts in requests.
  - `REJECTED` and `AUTH_REQUIRED` are never produced.
  - External callers have no Endpoint identity, so the adapter acts only as this Endpoint's local operator.

### A10 — New errors

| Code | Name | Meaning |
| --- | --- | --- |
| 96 | `unknown_agent` | not defined, or not available to this caller |
| 97 | `unknown_task` | not found, not owned, or not this Agent's |
| 98 | `agent_unavailable` | the Agent is stopped (`submit` while stopped) |
| 99 | `task_ended` | the operation needs a non-terminal task (`send`) |
| 100 | `unknown_artifact` | no artifact of that name on that task |

## Local API and CLI

- **Local API.**
  - `agent_define`, `agent_undefine`, `agent_start`, `agent_stop`: mutations with RequestId replay.
  - `agent_list`: definitions, instance state and task counts.
  - `agent_task_list {agent?, after?}`, `agent_task_inspect {task_id}`, `agent_task_events {task_id, after}`.
  - `agent_task_submit`, `agent_task_send`, `agent_task_cancel`: local creator, with replay.
  - `agent` joins `service_expose` and `service_unexpose` (selector `agent`), `space_grant` gains `agent`, and `stream_open` gains `agent`.
- **CLI.**

  ```text
  ma2a agent define AGENT [--description D] [--tool FAMILY]… [--max-tasks N] [--cwd D] [--env K=V]… -- PROGRAM ARGS…
  ma2a agent undefine|start|stop AGENT        ma2a agent list
  ma2a agent submit [--endpoint ID] AGENT [--text T]… [--data JSON]… [--request-id HEX] [--wait]
  ma2a agent tasks [--endpoint ID] [AGENT]
  ma2a agent task|watch|cancel [--endpoint ID --agent A] TASK      (watch: --after SEQ, reconnects)
  ma2a agent send [--endpoint ID --agent A] TASK --text T
  ma2a agent artifact --endpoint ID --agent A TASK NAME --output FILE
  ma2a agent discover --endpoint ID
  ma2a a2a serve [--listen ADDR]
  ```

## Crate layout

- **`ma2a-core`**: the `agent` capability, exposure kind and `ServiceAccess`; `ServiceKind::Agent`; `service/agent*`: ids, requests, replies, cards, task summaries and details, events, messages, artifact refs (canonical CBOR); the new errors.
- **`ma2a-store`**: schema 9. Exposure kind 5. Tables `agents`, `agent_instances`, `agent_outbox`, `agent_tasks` (unique `(creator, request_id)`), `agent_task_events`, `agent_artifacts` and `agent_tool_calls`, with guarded transitions.
- **`ma2a-agent`** (new crate):
  - the adapter protocol types as serde JSON;
  - a small Rust SDK: a stdio loop, deduplication of deliveries, tool calls as futures;
  - `ma2a-example-agent`, a deterministic reference Agent: a command interpreter over the tools (`echo`, `fetch`, `exec`, `delegate`, `ask`, `sleep`). It is an example implementation, not an LLM.
- **`ma2a-net`**: `AGENT_ALPN`.
- **`ma2a-runtime`**:
  - `agents/`: the manager (instances, link, assignment, cancellation, convergence, retention);
  - the Agent service host;
  - the tool bridge;
  - local API commands and web routes.
- **`ma2a-app`**: the `agent` and `a2a` CLI, the A2A adapter server, and E2E.
- **`web`**: a read-only Agents card (definitions, instances, recent tasks) on the Services route.

## Deterministic fault points (debug builds only)

`MA2A_TEST_AGENT_FAULT=<point>` SIGKILLs the daemon (`ma2a_services::supervisor::crash()`) at one of these points:

- `after_task_created`: the task is committed and not yet assigned;
- `after_assigned`: `running` is committed and the delivery is not yet written;
- `after_tool_recorded`: the tool call is `pending` and not yet executed;
- `before_tool_result`: the tool has executed and its result is not yet recorded.

Unit tests drive the same windows in process.

## Acceptance gate (real multi-Endpoint E2E)

- Agent A on Endpoint A and Agent B on Endpoint B (`ma2a-example-agent`).
- A local task makes A submit to B (`delegate`).
- B streams progress, uses `transfer.get` against a root exported by Endpoint A, and publishes an artifact.
- A fetches the artifact with `agent.artifact` and completes.
- The operator retrieves A's result and artifact.
- A network interruption (the target daemon restarts) during `watch` leads to a reconnect from `after`, with no lost state and no duplicate events.
- The same task-creation RequestId creates no duplicate task.
- An Agent process crash fails its active tasks with `agent_lost`, and the instance restarts.
- A Runtime restart reconciles: the instance is re-adopted and the running task completes.
- The adversarial tests cover:
  - an `AgentId` not exposed to the caller's Space, which is `unknown_agent`;
  - a grant in one Space with the exposure in another, which is denied;
  - another creator's task, which is `unknown_task`;
  - revocation, which cancels tasks with `unauthorized`;
  - a stopped Agent;
  - limits;
  - a forged `from_agent`, which is attribution only.
- The A2A adapter: an A2A JSON-RPC client sends, streams, gets and cancels against local and remote Agents.

Phase 1, 2 and 3 E2E remain green.

## Freeze record (as built)

Phase 4 is frozen on local `master`. The commits after the Phase 3 freeze (`59258db`) are:

- `docs`: the Phase 2–5 roadmap and this plan;
- `feat(core)`, `feat(agent)`, `feat(store)`, `feat(runtime)`, `feat(agent)` (bounded local
  artifact reads), `feat(a2a)`, `feat(web)`: Phase 4 itself;
- `fix(control)`: a pull returns a member's first address record, found by the E2E lab;
- `test(agent)` ×2: the acceptance, recovery, and adversarial E2E;
- `fix(a2a)`, `fix(process)`, `fix(agent)` ×4, `fix(app)`: fixes from E2E work and the freeze
  audit;
- `docs(protocol)` ×2, `docs(agent)`, and this documentation commit.

**Audit fixes.** The fresh audit found 2 blockers and 10 should-fix issues. Both blockers and every
should-fix issue are fixed:

- (blocker) a `tool` line with a bad name or args over 16 KiB, or a tool answer over 64 KiB, broke
  a Store `CHECK`, so the line was retried forever or the call stayed pending forever;
- (blocker) ended Agent instances filled the local Endpoint's 16 retained Processes, so after 16
  instance lifetimes no Agent could start;
- `agent_stop` left the instance live, and a launch racing a stop left its Process running;
- a redefined Agent reused its predecessor's instance-start RequestIds;
- a workspace FIFO could hang a tool call and daemon shutdown;
- publishing stored content replaced the blob, changing the identity a resumed download relies on;
- proxy listeners of tasks ended by a stop or a bound were never released;
- alternating non-terminal states escaped the durable-event bound;
- unread deliveries were unbounded, and an `ack` beyond the last delivery pruned the outbox;
- an artifact whose event would exceed the bound was still published;
- the A2A adapter: `CancelTask` on an ended task, a streamed continuation closing on the stale
  `waiting` state, an unbounded stop, local error codes, and `ListTasks` paging;
- `ma2a agent artifact --output` shared one partial file between `x.txt` and `x.bin` and could
  remove one it did not create.

One nit is left open: the A2A adapter's per-stream event channel is unbounded; it is drained by
the SSE writer and closed with the client.

**Documentation.** `docs/protocol/agent-v1.md` (the Agent service, adapter protocol, tool bridge,
limits, and every deviation from this plan), `docs/protocol/a2a-adapter.md`, and the Phase 4
sections of `docs/protocol/local-api-v1.md`.

**Test-only items.** `MA2A_TEST_AGENT_FAULT` (debug builds only), the deterministic reference
Agent `ma2a-example-agent` (not a language model), the E2E lab's Agents, the test HTTP client
`crates/ma2a-app/tests/agent_e2e/http_client.rs`, `#[cfg(test)] Lines::cursor`, `mkfifo` in a
workspace unit test, and the Agents data in `web/src/test/services-fixture.ts`.

**Verification at freeze**, on Linux x86_64:

| Check | Result |
| --- | --- |
| `cargo fmt --all -- --check` | clean |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | clean |
| `cargo xtask check-loc` | clean |
| `cargo deny check` | clean |
| `cargo machete` | clean |
| `cargo test --workspace --no-fail-fast` | 868 passed, 0 failed |
| `cargo test -p ma2a-app --test agent_e2e`, three runs | 12/12 each, no leftover Agent or adapter process |
| `--test process_e2e --test services_e2e --test e2e`, run again | 91/91 passed |
| `ma2a-core`, `ma2a-services` (`--features blake3/pure`) and `ma2a-agent` clippy for `aarch64-apple-darwin` and `aarch64-pc-windows-msvc` | clean |
| web: `biome check`, `tsc --noEmit`, `bun test` | clean, 144 tests passed |
