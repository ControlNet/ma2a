# Agent Service v1

Phase 4 adds one Endpoint-targeted service family, Agent. An **Agent** is a logical workload that
an Endpoint hosts next to its Processes. A caller submits **tasks** to it, follows their events,
and downloads the **artifacts** they publish. An Agent is never a cryptographic identity: remote
authentication stays Endpoint to Endpoint, and authorization still needs one complete Space path.
An `AgentId` only attributes and routes: caller Endpoint → target Endpoint → target Agent.

There is no delegation of authority, no multi-agent scheduler, and no language model in Core. The
Agent program is any executable that speaks the [adapter protocol](#agent-adapter-protocol) on
its stdio; `ma2a-example-agent` is a deterministic reference implementation.

Agent reuses the Services v1 frame layer, status envelope, and caller-side stream plane unchanged
(`docs/protocol/services-v1.md`), and runs every Agent instance as a Phase 3 Process
(`docs/protocol/process-v1.md`). This document only specifies what Agent adds. The local operator
commands are in `docs/protocol/local-api-v1.md`, and the external A2A v1.0 adapter is in
`docs/protocol/a2a-adapter.md`.

## Trust model

- **Endpoints authenticate, Agents do not.** The target authorizes the authenticated QUIC peer,
  never an Agent name. A submitting Agent's name (`from_agent`) is recorded and shown for
  attribution only.
- **Tools carry the hosting Endpoint's authority.** An Agent's tool calls run as same-user
  stream-plane clients of its own Runtime, with exactly the authority, replay, and audit of the
  CLI. An Agent never holds more authority than the Endpoint that hosts it.
- **The adapter is not a security boundary.** The Agent program runs as the daemon's OS user, and
  the local IPC server admits every peer with that effective user as the local operator
  (`docs/private-ipc.md`). The tool family policy is a guardrail and attribution, not isolation.

## Transport

The ALPN is exactly `ma2a/agent/1`, and the service code in the `open` header is `8`
(`ServiceKind::Agent`). It is negotiated like the other service ALPNs: only while the Runtime holds
at least one Space, and regardless of exposure.

- **Streams.** One operation uses one QUIC bidirectional stream, with the Services v1 frames
  `open`, `accept`, `reject`, `data`, `outcome`, and `cancel`.
- **Deadlines.** The `open` frame must arrive within 15 seconds. A `watch` lasts until its task
  has ended and every event was delivered, or until the caller cancels.
- **Concurrency.** At most 32 streams per peer and 128 Runtime-wide are open at once, so a caller
  can hold several long-lived `watch` streams. A stream over either bound is rejected with
  `concurrency_exceeded` before any byte is read.

`reject` means that nothing happened, and `outcome` means that the operation was accepted.

## Authorization

The authenticated QUIC peer is the caller. Every operation that names an Agent needs
`ServiceAccess::Agent(agent)`: **one single Space** in which

1. both the caller and this Runtime are current members;
2. the caller's signed member grant holds `agent` (capability bit value `256`); and
3. this Runtime's local exposure for that same Space has kind `agent` (Store exposure kind `5`)
   with exactly that `AgentId` as its selector.

`agents`, which names no Agent, needs `ServiceAccess::AgentAdmission`: one Space with both members,
the `agent` grant, and at least one exposed Agent. Grants and exposures from different Spaces
never combine, and the authorizing Space is never named or revealed.

The check runs twice, both from durable state at call time:

- **Coarse admission**, before the `open` frame, for `AgentAdmission`. A caller that fails it
  receives `reject(unauthorized)`.
- **Fine decision**, after decoding the `open`, for the exact access of the operation. A caller
  that fails it receives `reject(unknown_agent)`, exactly like an Agent that is not defined. An
  undefined Agent, an Agent not exposed in a Space the caller shares, and an Agent the caller
  lacks the grant for are indistinguishable. `agents` names no Agent, so a caller that fails its
  fine decision, because its access was withdrawn after admission, receives
  `reject(unauthorized)`.

The operator consents with `ma2a service expose agent SPACE_REF AGENT`, once per Space and Agent.
The Space owner grants the bit with `space grant … --agent`, repeating every other flag the member
should keep: a grant replaces all eight bits, so a flag left out is cleared.

**Creator ownership.** A remote caller sees and manages only the tasks its own Endpoint created.
Every task operation names its Agent, and the task must belong to that Agent. A task that does not
exist, was created by another Endpoint, or belongs to another Agent answers `unknown_task`, in all
three cases alike. The local operator sees every task through the local API. A caller takes no
answer that names a task of another creator, the tasks of a page included, as the answer to its
request.

**Attribution only.** `from_agent` in `submit` is recorded in the task, shown in its summary, and
passed to the Agent as `from.agent`. It never authorizes anything, and the target never checks it.

While a `watch` runs, it is re-authorized after every durable change (checked every second). One
that no longer passes ends with outcome `unavailable`. Tasks are ended separately; see
[Withdrawn authorization](#withdrawn-authorization).

## Identifiers and bounds

| Name | Definition |
| --- | --- |
| `AgentId` | 1–63 bytes of `[a-z0-9-]`, starting with a lowercase letter and not ending with `-`; a CBOR text string; unique per hosting Endpoint. A Runtime defines no Agent named `blobs` or a Windows device name (`con`, `nul`, `com1`, …) on any host, and never starts one defined before this rule; one found started is stopped when the Runtime starts, as if by the operator, so its unfinished tasks fail with `agent_stopped` and a new one is `reject(agent_unavailable)` |
| `AgentTaskId` | 16 random bytes: a CBOR byte string of length 16, and 32 lowercase hexadecimal characters in text |
| `RequestId` | the 16-byte `request_id` of the `open` header |
| `seq` | a task's event sequence number, an unsigned 32-bit integer starting at 1 |

| Constant | Value |
| --- | --- |
| `MAX_MESSAGE_PARTS` | 16 parts per message (at least 1) |
| `MAX_MESSAGE_BYTES` | 12,288 bytes of part content per message, all parts together |
| `MAX_AGENT_DESCRIPTION_LEN` | 256 bytes |
| `MAX_AGENT_TASKS` | 16: `max_tasks` is 1–16 |
| `MAX_AGENT_PAGE` | 16 cards per `agents` page |
| `MAX_TASK_PAGE` | 32 summaries per `tasks` page |
| `MAX_ARTIFACT_NAME_LEN` | 128 bytes |
| `MAX_MEDIA_TYPE_LEN` | 127 bytes |
| `MAX_TASK_ARTIFACTS` | 16 artifacts per task |
| `MAX_TASK_NOTE_LEN` | 1,024 bytes of error or progress text |

## Open bodies

The `open` header is the Services v1 map with service `8`. Its body (key `4`) is a tagged CBOR
array. Every value is canonical: definite lengths and shortest-form integers. Unknown tags, a wrong
array length, and out-of-bound values are all `invalid_input`. The whole header is at most 16 KiB.

| Tag | Operation | Body | Class |
| --- | --- | --- | --- |
| 0 | `agents` | `[0, after: AgentId / null]` | read |
| 1 | `submit` | `[1, [agent, message, from_agent: AgentId / null]]` | mutation |
| 2 | `send` | `[2, agent, task, message]` | mutation |
| 3 | `get` | `[3, agent, task]` | read |
| 4 | `tasks` | `[4, agent, after: AgentTaskId / null]` | read |
| 5 | `watch` | `[5, agent, task, after: uint32]` | read |
| 6 | `cancel` | `[6, agent, task]` | mutation |
| 7 | `artifact` | `[7, agent, task, name: tstr, offset: uint, identity: [size, modified_ns] / null]` | read |

The class decides caller-side replay (see [Caller side](#caller-side-replay-and-lost-streams)).

**Message.** A message is `[role, [[kind, content: tstr]…]]`. `role` is the text `"user"` or
`"agent"`. `kind` is `0` for text and `1` for data, whose content is JSON text. A message holds
1–16 parts, at most 12,288 bytes of content in total, and no NUL character. Larger payloads must be
artifacts. A message that a caller sends (`submit`, `send`) must have role `user`, and every data
part must parse as one JSON value; otherwise the operation is `reject(invalid_input)`.

## Accept payloads

| Form | Encoding | Sent by |
| --- | --- | --- |
| Submitted | `[0, task: AgentTaskId, existing: bool]` | `submit`; `existing` is `true` when this RequestId already created the task |
| Watching | `[1, state: uint, next: uint]` | `watch`; the task state now, and `after + 1` |
| Admitted | `[2]` | `agents`, `send`, `get`, `tasks`, `cancel` |

`artifact` answers with the Transfer v1 download `accept` instead (see [Artifacts](#artifacts)).

## Outcome payloads

The outcome envelope is the Services v1 map `{0: status, 1: result / null}`. `status` is `0` for
success, and otherwise one of the status codes with a `null` result. A successful result is:

| Tag | Result | Encoding | Operation |
| --- | --- | --- | --- |
| 0 | Cards | `[0, [card…], next: AgentId / null]` | `agents` |
| 1 | Summary | `[1, summary]` | `submit`, `send`, `cancel`, end of `watch` |
| 2 | Details | `[2, details]` | `get` |
| 3 | Page | `[3, [summary…], next: AgentTaskId / null]` | `tasks` |

**Card.** `[id: AgentId, description: tstr, available: bool, max_tasks: uint]`. `available` is
`true` while the operator has the Agent started, so a `submit` is accepted. `description` is at
most 256 bytes without NUL, and `max_tasks` is 1–16.

**Summary.** An 11-item array, content-free:

```text
[id: bstr(16), agent: tstr, creator: bstr(32), from_agent: tstr / null,
 request_id: bstr(16), state: uint, reason: uint / null, created_ms: uint,
 started_ms: uint / null, ended_ms: uint / null, last_seq: uint]
```

- **creator** is the Endpoint that submitted the task, or the hosting Endpoint for local tasks.
- **state** is `queued` 0, `running` 1, `waiting` 2, `completed` 3, `failed` 4, or `cancelled` 5.
- **reason** is `requested` 0, `unauthorized` 1, `agent_failed` 2, `agent_lost` 3,
  `agent_stopped` 4, or `limit` 5.
- **times** are Unix milliseconds. `started_ms` is set when the task first becomes `running`.
  `ended_ms` is set exactly when the state is terminal.
- **last_seq** is the sequence number of the newest event.

**Details.** `[summary, input: message, result: message / null, error: tstr / null,
progress: tstr / null, [artifact…]]`. `input` is the submitted message, `result` the inline result
of a completed task, `error` the failure text the Agent reported, and `progress` the newest
progress text. Artifacts are listed in publication order.

**Artifact reference.** `[name: tstr, media_type: tstr, size: uint, blake3: bstr(32)]`.

- `name` is 1–128 bytes of `[A-Za-z0-9._-]`, not starting with `.`, and unique within its task.
- `media_type` is at most 127 bytes of printable ASCII of the form `type/subtype`.

## Operations

**`agents`.** This returns one page of cards for the Agents this caller may use, in name order
strictly after `after`. A defined Agent is listed only when the caller passes the full
`Agent(id)` check for it. `next` is the last name of the page when more exist, or `null`.

**`submit`.** The target records the task and its RequestId binding, sends `accept` Submitted,
then `outcome` Summary. For a new task, the Summary is the task as recorded, `queued`; assignment
to an instance follows. For a retry, it is the existing task as it now is. See
[RequestId binding](#requestid-binding).

**`send`.** This records one more message from the creator and delivers it to the Agent. The task
may be `queued`, `running`, or `waiting`. A message sent to a queued task is delivered together
with the task when it is assigned. A terminal task, or one whose cancellation was already
requested, is `reject(task_ended)`. The outcome is the Summary. A message that would take the task
past its durable-event bound fails the task with `limit` instead of reaching it; that send took
effect, so it is answered `accept` Admitted and then the outcome error `task_ended`, not a
rejection. The local API answers it `conflict` as a committed failure, so its request ID is not
released for another try.

**`get`.** This returns Details.

**`tasks`.** This returns one page of the caller's own tasks of the named Agent, in ascending task
ID byte order, strictly after `after`. `next` is the last ID of the page when more exist, or
`null`.

**`watch`.** See [Events and watch](#events-and-watch).

**`cancel`.** This cancels a task and answers once the task has ended:

- a `queued` task is cancelled at once;
- a `running` or `waiting` task has its cancellation recorded and `cancel` delivered to its
  instance, and ends as described in [Cancellation](#cancellation);
- an already terminal task answers its current Summary.

The target waits at most 60 s for the end, and stops waiting when it shuts down or the caller's
authorization is withdrawn. A task still live at that point, or one whose end can no longer be
read, answers `accept` Admitted followed by outcome `outcome_unknown`; the cancellation stays
recorded and still ends the task.

**`artifact`.** See [Artifacts](#artifacts).

**Refusals.** Every refusal decided before an effect is a `reject` frame: `invalid_input`,
`unknown_agent`, `unknown_task`, `unknown_artifact`, `agent_unavailable`, `task_ended`,
`conflict`, `concurrency_exceeded`, and `unavailable` (Store failure). The failed outcomes after
`accept` are the ones described with each operation: `send`'s `task_ended` at the durable-event
bound, `cancel`'s `outcome_unknown`, the ends of `watch`, and `artifact`'s download failures such as
`content_changed`, `integrity_mismatch`, and `timed_out`.

## Task state machine

The state is durable in the target's Store:

| From | To |
| --- | --- |
| `queued` | `running`, `failed`, `cancelled` |
| `running` | `waiting`, `completed`, `failed`, `cancelled` |
| `waiting` | `running`, `completed`, `failed`, `cancelled` |

`completed`, `failed`, and `cancelled` are terminal. Every change an Agent reports is checked
against this table and ignored if it is not listed or does not change the state. The Store also
requires an instance generation for `running` and `waiting`, and ties `ended_ms` to the terminal
states.

| State | Meaning | Reason |
| --- | --- | --- |
| `queued` | recorded with its RequestId binding; not yet assigned to an instance | none |
| `running` | assigned to one instance generation and delivered to it | none |
| `waiting` | the Agent asked for input; `send` delivers it | none |
| `completed` | the Agent reported a result | none |
| `failed` | the task could not finish | `agent_failed`, `agent_lost`, `agent_stopped`, or `limit` |
| `cancelled` | the task was cancelled | `requested` or `unauthorized` |

| Reason | Cause |
| --- | --- |
| `agent_failed` | the Agent reported `failed` |
| `agent_lost` | the instance holding the task ended |
| `agent_stopped` | the operator stopped the Agent; this fails every unfinished task, queued ones included |
| `limit` | the task reached its event or tool-call bound (see [Limits](#retention-and-limits)) |
| `requested` | a `cancel` |
| `unauthorized` | the creator no longer has a complete authorization path |

**Assignment.** A queued task is assigned only to the live, `ready` instance of a started Agent
that holds fewer than `max_tasks` running or waiting tasks, oldest first (creation time, then task
ID). Moving the task to `running` and adding its delivery to the instance outbox, with every
message sent while it was queued, commit in one transaction. Assignment runs every second, and also
right after a `submit` and after an Agent line that readies the instance or ends a task.

### Cancellation

- A cancellation requested for a `running` or `waiting` task records its reason and time, and
  delivers `cancel` to the instance that holds the task.
- The task ends as soon as the Agent reports a terminal state. The Agent may report `cancelled`
  only for a task whose cancellation was requested, and the task then ends with the requested
  reason.
- Otherwise the task ends `cancelled` with the requested reason once the 10 s grace period has
  passed. The grace period is checked every second. Tool calls of the task still open then are
  answered `task_ended` in the same transaction, so the Agent is never left waiting for them.
- A task whose cancellation was requested and whose instance ends, or whose Agent is stopped, ends
  `cancelled` with the requested reason, not `failed`.

## RequestId binding

The target's Store binds `(creator Endpoint, RequestId)` to one `AgentTaskId` with a unique key,
and records a fingerprint: BLAKE3 `derive_key("ma2a agent submit v1")` over the canonical submit
array `[agent, message, from_agent]`.

- **No binding yet.** The task row (`queued`), its first event, the binding, and the fingerprint
  commit in one transaction before any delivery to the Agent. An undefined Agent is
  `reject(unknown_agent)`, a stopped Agent `reject(agent_unavailable)`, and a request over a live
  task bound `reject(concurrency_exceeded)`, all with no effect.
- **Same fingerprint.** The target answers `accept [0, task, true]` and the task as it is now,
  whatever its state.
- **Different fingerprint.** The request is `reject(conflict)`.

The binding lives as long as the task row. Once retention removes the task, the same RequestId can
create a new task.

### Caller side: replay and lost streams

Callers use the Phase 2 stream plane unchanged. `submit`, `send`, and `cancel` reserve their
RequestId in the caller Runtime's durable replay table; reads never reserve. As in Process v1, a
completed operation's terminal frame is replayed verbatim to the same RequestId, a target `reject`
releases the reservation, and a stream lost after `open` was sent is settled as `outcome_unknown`.
Since only a mutation can be answered from replay, a caller refuses a read's outcome that came
without `accept`. It also refuses a `submit` Summary whose `request_id` is not the one it opened,
and a `cancel` Summary of a task that is not terminal.

The remediation for an unknown `submit` is `tasks`, whose summaries carry each task's
`request_id`. The target-side binding guarantees that no duplicate task is created. Each `send` and
`cancel` needs a fresh RequestId, because a reused one only replays the earlier outcome.

## Events and watch

Every task has a durable event log. `seq` starts at 1 and increases by one per event. Each event is
`[seq, at_ms, kind, …]`:

| Kind | Event | Encoding |
| --- | --- | --- |
| 0 | `state` | `[seq, at_ms, 0, state: uint, reason: uint / null]` |
| 1 | `message` | `[seq, at_ms, 1, message]`, from the creator (`send`) or the Agent |
| 2 | `progress` | `[seq, at_ms, 2, text: tstr]`, at most 1,024 bytes |
| 3 | `artifact` | `[seq, at_ms, 3, artifact reference]` |

The first event of every task is `state queued` with `seq` 1. The submitted input is not an event;
it is the `input` of Details. When the Agent reports a state together with a message, the message
is recorded as its own `message` event, immediately before the `state` event.

- **Durable events.** `state`, `message`, and `artifact` events are never pruned. A task holds at
  most 256 of them: a `message`, `artifact`, or non-terminal `state` event that would exceed this
  fails the task with reason `limit` instead. A terminal `state` event is always recorded, so the
  end of a task is never lost. An artifact whose event would exceed the bound is not published.
  A task failed this way has its open tool calls answered `task_ended` (see
  [Tool bridge](#tool-bridge)).
- **Progress.** Only the newest 64 `progress` events of a task are retained; older ones are
  deleted when a new one is recorded. The newest progress text is also kept in Details.

**`watch`.**

1. The target sends `accept` Watching with the task state and `next = after + 1`.
2. It sends every retained event with `seq > after` in order, one `data` frame on channel 1 per
   event, whose payload is the event encoding above. It reads them in batches of 64.
3. It keeps sending each new event as it is recorded.
4. The outcome is Summary once the task is terminal and every event was delivered. The task is
   read before its events, so no event recorded before the end is missed.

A `cancel` frame, or the end of the caller's stream, ends the watch with outcome `cancelled`; any
other frame from the caller ends it with outcome `invalid_input`. Runtime shutdown or withdrawn
authorization ends it with outcome `unavailable`, also while history is still being sent. A task removed by
retention while it is watched ends it with outcome `unknown_task`.

**Reconnect.** A watcher that reconnects with `after` set to the last `seq` it received gets every
retained event after it, with no duplicates. Gaps can appear only among `progress` events that were
pruned in between, so the first event delivered may have a `seq` above `next`. `ma2a agent watch`
and `submit --wait` reconnect this way after a watch fails with `unavailable`, `unreachable`,
`timed_out`, `outcome_unknown`, or `cancelled`, once a second. Reopening the watch is then retried
the same way while it fails for one of those reasons or because this Runtime is away for now, as
while it restarts. A Runtime that answers with another local API version, or a local IPC path
that is not this user's, is not away for now. That failure, any other failure, or a failure of
the first watch to open, ends the command.

A caller treats a `watch` as malformed (`invalid_input`) when it is accepted as anything other
than Watching with `next = after + 1` (saturating at the maximum `seq`), when an event's `seq` is
not above `after` or not above the event before it, when any event follows the task's terminal
`state` event, or when its outcome is anything other than the Summary of the watched Agent's task,
created by the caller's own Endpoint, once ended. That Summary's `last_seq` must be the `seq` of
the last event delivered, and that event the `state` event of the state and reason the Summary
reports. When no event was delivered, `last_seq` must be between 1 and `after`. The terminal
`state` event is never pruned, so a watch that did not deliver it is missing the task's end. A
caller that reconnects after it already received that terminal event at `after` takes no event at
all, and only the Summary of that same end.

## Artifacts

**Publication.** An Agent publishes a file from its workspace with the `artifact.publish` tool. The
daemon:

1. resolves the path inside the workspace and requires a regular file, opened without blocking;
2. copies it into a staging file in the content-addressed store `<state>/agents-v1/blobs/` (mode
   `0700`), hashing it with BLAKE3 as it goes;
3. fsyncs the copy, renames it to `<blake3 hex>` and, on Unix, fsyncs `blobs/` and the directory holding it before the reference commits, so identical content is stored once;
4. records the artifact reference and an `artifact` event, while the task is not terminal, and
   the tool call's answer in the same commit, so a restart never runs a recorded publication
   again (which would find its own name taken).

| Bound or failure | Result |
| --- | --- |
| content over 256 MiB | `too_large` |
| missing source | `not_found` |
| name already used by the task, or a 17th artifact | `conflict` |
| task already terminal | `task_ended` |
| invalid name or media type, checked before anything is copied | `invalid_input` |
| source that is not a regular file, or a workspace path that escapes | `invalid_tool` |
| any other failure to read the source or write the blob directory | `internal` |

The media type defaults to `application/octet-stream`.

**Retrieval.** `artifact` serves the blob through the unchanged Transfer v1 download path
(`docs/protocol/services-v1.md`). An unknown task is `reject(unknown_task)`, and a name the task
did not publish is `reject(unknown_artifact)`. Then:

1. `accept` is the Transfer download accept `[0, [size, modified_ns]]` of the blob file.
2. Content follows as `data` on channel 1 from `offset`.
3. The outcome is the Transfer v1 result `{bytes, size, blake3}`, hashed over the whole content.

As in Transfer v1, an `offset` above the size is `invalid_input`, a resumed download names the
identity it continues, and a different identity is `reject(precondition_failed)`. The blob must
still be the content the task recorded: a blob of another size is `reject(integrity_mismatch)`,
and one whose content hashes to anything but the recorded BLAKE3 ends with a failed outcome of
`integrity_mismatch`. The client verifies BLAKE3 over its final file.

**Local reads.** The local operator reads an artifact of any task in chunks through the local API:
`agent_task_artifact_read {task_id, name, offset}` returns at most 32,768 bytes from `offset` as
standard base64, with the reference and `eof`. A reader verifies the whole content against the
returned hash. The local API reports an unknown task or artifact as `not_found`. Like the
`artifact` operation, a local read takes only a regular blob file of the recorded size and never
follows a link in its place; anything else is `internal`.
`ma2a agent artifact … --output FILE` uses these chunks for a local task and the `artifact`
operation for a remote one. Like `transfer get`, it writes a partial file in the private
`.ma2a-downloads` directory beside `FILE`, holding that directory and `FILE`'s from the start,
verifies the content, and publishes that very file as `FILE` without replacing anything, so an
existing `FILE` is never overwritten and a concurrent download to the same `FILE` is refused
(see the resumable-download rules in `services-v1.md`).

**Publication.** `artifact.publish` copies the file to a staging file in the blob directory while
hashing it. A blob of that hash that is already stored as a regular file, and still holds that
size and hash, is kept untouched and the copy is discarded, so the blob's content identity never
changes and a resumed download always continues. Otherwise, including when the stored blob was
damaged, cannot be read, or is a link, which a download never follows, the copy is renamed to the
blob.

**Collection.** Every 60 s, after expired tasks are removed, every file in the blob directory that
no retained task references and that is at least 10 minutes old is deleted. Collection and the
placing-and-recording step of a publication exclude each other, so a blob kept as already stored
is never collected before the new reference is recorded.

## Agent definitions and instances

**Definition.** The local operator defines each Agent (`agent_define`):

- a description of at most 256 bytes;
- the program, arguments, optional absolute `cwd`, and environment, bounded like a Process
  `spec`; environment names starting with `MA2A_AGENT_` are `invalid_input`, and so is a
  definition whose instance start, with the three adapter variables and the workspace added,
  would break a Process bound (at most 61 defined variables therefore fit);
- the allowed tool families, as the bit set in [Tool bridge](#tool-bridge);
- `max_tasks`, the concurrent tasks per instance, 1–16. The local API has no default; the CLI
  defaults `--max-tasks` to 1.

A new Agent starts stopped. A definition can be replaced or removed only while the Agent is
stopped. Removing it keeps its finished tasks until they expire.

**Start and stop.** `agent_start` enables the Agent, and its first instance starts at once.
`agent_stop` disables it in one transaction: every unfinished task fails with `agent_stopped`,
the live instance ends, and its outbox is dropped. The instance Process is then terminated with a
5 s grace period. A launch that races a stop finds its instance ended when it binds the Process,
and terminates that Process the same way.

**Instance.** An instance is one incarnation of a started Agent: a Phase 3 Process that the local
Endpoint creates for itself, with a generation number. Each start takes the next generation, and
a definition created anew (also after its name was removed) counts on from the Store's current
revision. A generation is therefore never reused for one Agent name, and a late callback of a
removed definition's instance can never act on its successor. Generations of different Agents may
be equal, so every task-affecting adapter line is also checked against the task's Agent. An Agent
has at most one live instance.

- It is started with stdin as a pipe, a 16 MiB log limit, and the label `ma2a.agent` set to the
  `AgentId`.
- Its `cwd` is the definition's, or else the workspace `<state>/agents-v1/<agent>/workspace`,
  created with mode `0700`.
- Its environment is the definition's plus `MA2A_AGENT_ID`, `MA2A_AGENT_WORKSPACE`, and
  `MA2A_AGENT_PROTOCOL=1`.
- The start's RequestId is the first 16 bytes of BLAKE3 `derive_key("ma2a agent instance start
  v1")` over `agent ‖ 0x00 ‖ defined_at_ms ‖ generation` (both u64 BE), where `defined_at_ms` is
  the creation time of the definition. A crash between the Process commit and the instance commit
  therefore never starts a second Process for one generation, and an Agent removed and defined
  again never replays its predecessor's start.
- The local Endpoint's own Processes are exempt from Process withdrawal convergence, and remote
  callers never see them, because no remote peer can be the local Endpoint.

**Link.** One link per live instance attaches to its Process as the single stdin writer, and reads
its output from the instance's durable cursor. It retries reaching the supervisor with backoff from
250 ms up to 10 s.

**Ready.** A new instance must announce `ready` within 30 s of its start. Otherwise its Process is
terminated with a 1 s grace period, and the instance ends. Assignment starts only once it is ready,
and the ready flag is durable.

**Instance end.** When the instance Process ends, whether the Agent crashed, exited, or was
stopped, the instance is concluded in one transaction:

- every `running` or `waiting` task it held fails with `agent_lost` (or ends `cancelled`, see
  [Cancellation](#cancellation));
- its outbox is dropped;
- queued tasks stay queued for the next instance.

**Restart backoff.** While the Agent stays started, the next generation starts after a pause: 1 s
after the first end, then double the previous pause, at most 60 s. An instance that ran for at
least 60 s resets the pause to 1 s. An instance that cannot start at all (for example over a
Process limit, or on a Runtime without Process hosting) is concluded at once and follows the same
backoff. The backoff is held in memory; `agent_start` and a Runtime restart reset it.

**Runtime restart.** Daemon shutdown never stops an instance: its Process keeps running under its
supervisor. At the next start the Runtime:

1. links every live instance again; Phase 3 re-adopts its Process through the ownership proof;
2. resumes reading from the durable cursor, so no Agent line is applied twice or skipped;
3. writes every unacknowledged delivery again, in order;
4. re-executes every pending tool call of a live task (see [Tool bridge](#tool-bridge)).

The tasks simply continue.

## Agent adapter protocol

The Agent speaks JSON lines over stdio: one compact JSON object per line, tagged by `"t"`, with
unknown fields refused. The Agent writes to stdout; stderr is free-form log. The Runtime writes to
the Agent's stdin. The Rust crate `ma2a-agent` holds the types, the tool vocabulary, and a small
SDK.

**Messages** in JSON are `{"role": "user" | "agent", "parts": [part…]}`, where a part is
`{"kind": "text", "text": "…"}` or `{"kind": "data", "data": <any JSON value>}`. They are bounded
like CBOR messages.

### Agent → Runtime

| `t` | Fields | Effect |
| --- | --- | --- |
| `ready` | `protocol: 1` | the instance is ready for tasks; any other version is ignored |
| `ack` | `seq` | every delivery up to and including `seq` was processed |
| `state` | `task`, `state`, `message?`, `error?` | the task moves to `running`, `waiting`, `completed`, `failed`, or `cancelled` |
| `message` | `task`, `message` | one Agent message (role `agent`) of the task's conversation |
| `progress` | `task`, `text` | transient progress |
| `tool` | `task`, `call`, `name`, `args` | one tool call, answered by `tool_result` with the same `call` |

- `task` is the 32-character hexadecimal task ID. A line about a task that this instance
  generation does not hold as `running` or `waiting` is ignored, except a `tool` line for a task
  it held that has ended, which is answered `task_ended` (see [Tool bridge](#tool-bridge)).
- In `state`, an Agent `message` is recorded as a `message` event, and with `completed` it also
  becomes the inline `result`. `failed` records `error` and reason `agent_failed`. A `message`
  that is invalid or not in role `agent` invalidates the whole line.
- `error` and progress `text` are cut to 1,024 bytes at a character boundary. Progress `text`
  with a NUL is ignored.
- `call` is 1–64 bytes of `[A-Za-z0-9._-]`, unique per task. A repeated `call` is ignored and
  never runs twice.
- A `tool` line whose `name` is not 1–64 bytes, or whose `args` encode to more than 16,384 bytes,
  is ignored. The SDK refuses such a call itself with `too_large`, without writing it.
- `ack` only moves forward, and acknowledged deliveries leave the outbox. An `ack` of a `seq` that
  was never delivered is ignored.

**Exactly-once application.** The link cuts stdout into lines by content-byte cursor over the
Process log. Each complete line is applied in one Store transaction together with the advance of
the instance's output cursor to just past its newline. A line at or before the stored cursor, or
from an instance that is no longer the live one, changes nothing. Every Agent line therefore takes
effect exactly once across daemon crashes. Stderr bytes only advance the cursor. When log
retention dropped output before the link read it, the line that was cut is dropped.

**Protocol violations.** A line over 65,536 bytes (without its newline), a malformed line, an
unknown tag, a bad task ID or call ID, and a line naming a task the instance does not hold only
advance the cursor. A trailing carriage return is accepted. A violation never fails another task.
A line that would break a Store bound (a length or count `CHECK`) is applied as a violation: its
transaction is rolled back and only the cursor advances, so no Agent line can stall the link.

### Runtime → Agent

| `t` | Fields | Delivers |
| --- | --- | --- |
| `task` | `seq`, `task`, `from: {endpoint, agent?}`, `message` | a newly assigned task; `endpoint` is the creator's Endpoint ID as 64 lowercase hexadecimal characters, and `agent` is the attribution-only `from_agent` |
| `message` | `seq`, `task`, `message` | the creator's next message (from `send`) |
| `cancel` | `seq`, `task` | a cancellation request, or the end of a task the Runtime failed at a limit |
| `tool_result` | `seq`, `task`, `call`, `ok` or `error: {code, message}` | the answer to one tool call |

**At-least-once delivery.** Each delivery gets an instance-scoped `seq`, starting at 1 for every
generation. It is first stored in the instance outbox, in the same transaction as the change that
caused it; the outbox stores only references, and each line is rebuilt from durable rows, so a
re-sent line is byte-for-byte the first. The link writes a line as `data` frames of at most 64 KiB
each; a Runtime line is at most 16 MiB. On every new connection to the instance, the link first
writes an empty line, which ends any line a lost connection cut short, then re-sends every
unacknowledged delivery in order. A delivery whose task or call no longer exists is skipped.
While an instance holds 256 unacknowledged deliveries, no task is assigned to it; deliveries for
tasks it already holds are still added.

**Deduplication.** The Agent processes each `seq` once: it ignores any `seq` at or below the
highest it has processed, and acknowledges in order. Delivery is at least once and the effect
exactly once.

**SDK and reference Agent.** `ma2a_agent::sdk::serve` runs the stdio loop: it sends `ready`,
skips empty and malformed lines, deduplicates by `seq`, acknowledges, and gives each task a handle
with progress, messages, `ask`, cancellation, and tool calls as futures (call IDs `c1`, `c2`, …). A
handle dropped without an end reports `failed`, or `cancelled` after a cancellation request.
The SDK checks every message against the Runtime's message bounds (`Message::is_within_bounds`:
1–16 parts, 12,288 content bytes, no NUL) and the `agent` role before sending it, because the
Runtime ignores a whole line that carries any other message. Such a result or `ask` question
fails the task instead, and `message` returns `false` without sending. Once a task ended, its
handle sends no more progress or messages. A task delivered again while the
SDK holds it is ignored. At most 64 progress lines and 64 messages wait to be written: past
that, new progress is dropped and `message` returns `false` until the Runtime reads, while
acknowledgements, state reports, and tool calls are always kept.
`ma2a-example-agent` is a deterministic command interpreter built on it, not a language model. The
first word of a task's text is the command:

| Command | Effect |
| --- | --- |
| `echo TEXT` | completes with `TEXT` |
| `ask QUESTION` | waits for the creator's answer, then completes with it |
| `sleep MS` | reports progress every 100 ms for `MS` ms, then completes |
| `progress N` | reports `N` progress events, then completes |
| `fail TEXT` | fails with `TEXT` |
| `crash` | exits the whole Agent process at once |
| `fetch ENDPOINT ROOT PATH NAME` | `transfer.get` a remote file, then publishes it as artifact `NAME` |
| `exec ENDPOINT PROGRAM ARGS…` | `exec.run`, then completes with its stdout |
| `delegate ENDPOINT AGENT TEXT…` | `agent.submit` and `agent.wait`, downloads and republishes the artifacts, and completes with the result |
| `tool NAME JSON` | calls one tool with raw arguments, and completes with its result as a data part |

## Tool bridge

An Agent asks for a tool with a `tool` line. The daemon:

1. records the call durably as pending, in the transaction that applies the line;
2. checks, when it executes the call, that the tool's family is allowed by the definition;
3. runs it as a same-user stream-plane client of its own Runtime;
4. records the result and delivers `tool_result` to the instance that holds the task.

| Family | Bit | Tools |
| --- | --- | --- |
| `files` | 1 | `files.stat`, `files.list` |
| `transfer` | 2 | `transfer.get`, `transfer.put` |
| `exec` | 4 | `exec.run` |
| `process` | 8 | `process.start`, `process.inspect`, `process.terminate` |
| `proxy` | 16 | `proxy.open` |
| `agent` | 32 | `agent.submit`, `agent.get`, `agent.wait`, `agent.send`, `agent.cancel`, `agent.artifact` |
| (always) | — | `artifact.publish` |

| Tool | Arguments | Result |
| --- | --- | --- |
| `files.stat` | `endpoint, root, path` | `{kind, size, modified_ms?, read_only}` |
| `files.list` | `endpoint, root, path, after?` | `{entries: [{name, kind, size}…], next?}` |
| `transfer.get` | `endpoint, root, path, to` | `{size, blake3}` |
| `transfer.put` | `endpoint, root, path, from, replace` (default `false`) | `{size, blake3}` |
| `exec.run` | `endpoint, program, args, cwd?, env?, timeout_ms?` | `{exit_code?, signal?, reason, stdout, stderr, truncated}` |
| `process.start` | `endpoint, program, args, cwd?, env?, name?` | `{id, state, reason?, exit_code?}` |
| `process.inspect` | `endpoint, id` | as `process.start` |
| `process.terminate` | `endpoint, id, grace_ms?` | as `process.start` |
| `proxy.open` | `endpoint, host, port` | `{listen}` |
| `agent.submit` | `endpoint, agent, message` | `{task, state, reason?}` |
| `agent.get` | `endpoint, agent, task` | `{task, state, reason?, result?, error?, progress?, artifacts}` |
| `agent.wait` | `endpoint, agent, task, timeout_ms?` | as `agent.get` |
| `agent.send` | `endpoint, agent, task, message` | `{task, state, reason?}` |
| `agent.cancel` | `endpoint, agent, task` | `{task, state, reason?}` |
| `agent.artifact` | `endpoint, agent, task, name, to` | `{size, blake3}` |
| `artifact.publish` | `name, path, media_type?` | `{name, media_type, size, blake3}` |

- **Remote only.** `endpoint` names the target Endpoint. Every tool except `artifact.publish`
  targets another Endpoint; the local API refuses a stream to this Runtime itself with
  `invalid_input`. The Agent is a local process and already has local OS access.
- **Remote paths.** `root` and `path` form the Services v1 path `ROOT:PATH`.
- **Workspace paths.** `to`, `from`, and `path` (of `artifact.publish`) are relative to the
  instance workspace `<state>/agents-v1/<agent>/workspace`. Only normal components are allowed:
  no `..`, no root, no backslash, and not empty. A file read must canonicalize to a path inside the
  canonical workspace and outside every `.ma2a-downloads` directory, so a symbolic link may be
  followed only within it and never into a download's helpers. The file is opened, the path is
  resolved again, and the open file must be the one it names; the tool reads that open file. A
  destination's parent directories are created as needed, and none of them may be a symbolic
  link; a destination that is a directory is refused. Downloads (`transfer.get`, `agent.artifact`)
  go to a partial file in the private `.ma2a-downloads` directory beside the destination, whose
  helper names have a fixed length whatever the destination is called, are verified, and are then
  renamed over the destination, replacing an existing file. Once the destination's directory is
  held, and before any helper file in it is made or changed, the destination is checked again and
  must still name that directory. A destination named
  `.ma2a-downloads` is refused. These checks keep a tool to the paths it was given; they are not a
  boundary against the Agent itself, which runs as the same OS user.
- **`exec.run`.** Stdin is empty, and nothing is interpreted by a shell. `timeout_ms` defaults to
  60 s and is at most 24 hours. `stdout` and `stderr` each carry at most the first 64 KiB as lossy
  UTF-8; when the whole answer would exceed the 64 KiB result bound, the longer stream is halved
  at a character boundary until it fits. `truncated` says that output was dropped. Shell is not offered as a tool; an Agent
  that needs a command uses `exec.run` or `process.*`.
- **`process.start`** starts a Process with stdin set to null and the default 4 MiB log limit.
  `process.terminate`'s `grace_ms` defaults to 5,000.
- **`proxy.open`** binds a listener on `127.0.0.1` at a free port and returns its address. Each
  accepted connection opens one Proxy stream to `host:port` with a 10-minute idle timeout. An
  invalid `host`, `port`, or `endpoint`, or this Runtime as `endpoint`, is refused before anything
  listens. A listener opens or relays at most 64 connections at once, the size of the local stream
  pool; later ones wait in the listen backlog. The listener lives in the daemon's memory; see
  [Known limitations](#known-limitations).
- **`agent.*`** act as this Endpoint. `agent.submit` sends the calling Agent's `AgentId` as
  `from_agent`, and its message must have role `user`. `agent.wait` follows the task with `watch`
  until it ends or `timeout_ms` passes (default and maximum 300,000), then answers its full view,
  which may still be live at the deadline. While waiting, `unavailable`, `unreachable`,
  `timed_out`, `outcome_unknown`, and `cancelled` are retried every second until `timeout_ms`
  passes. Each read of the task's view, in `agent.get` and `agent.wait`, is `timed_out` when the
  target has not answered within 30 s. A view read that ends after the deadline is the last one:
  its view or its error is the answer, so a wait ends at most 30 s after `timeout_ms`.
- **Read deadline.** `files.stat`, `files.list`, and `process.inspect` are `timed_out` when the
  target has not answered within 30 s, like a task view, and so are `transfer.get` and
  `agent.artifact` when the target has not accepted within 30 s; that 30 s also covers opening
  the stream through this Runtime. Mutations (`exec.run`, `transfer.put`, `process.start`,
  `process.terminate`, `agent.submit`, `agent.send`, `agent.cancel`) and a download's content
  have no such deadline: they wait for the target.

**Derived RequestId.** A call that opens a mutation (`transfer.put`, `exec.run`, `process.start`,
`process.terminate`, `agent.submit`, `agent.send`, `agent.cancel`) uses

```text
RequestId = blake3::derive_key("ma2a agent tool call v1", task ‖ call)[..16]
```

where `task` is the 16 task ID bytes and `call` the call ID bytes. Read-only calls use a fresh
random RequestId, and so does each proxy connection.

**Re-execution.** A call still executing when the daemon stops records no result and stays
pending. At Runtime start, every pending call of a `running` or `waiting` task runs again. A call
whose answer the Store fails to record keeps that answer and records it again, with backoff, until
the Store takes it or the daemon stops; it does not run again meanwhile. While the Runtime runs,
pending calls that are not executing are also run every 15 seconds, so a call whose run ended
early on a failed Store read is not left waiting for a restart.
A mutation re-opened under its derived RequestId is answered by the caller-side replay table: the
completed operation's terminal frame, or `outcome_unknown` when the earlier stream was lost. It
never acts twice. A replay carries only the terminal, never streamed output, so a replayed
`exec.run` answers `output_unavailable` instead of an empty result. A call that is already executing in this daemon is not started again, and a
completed call keeps its first result.

**Result bound.** A stored result, `{"ok": …}` or `{"error": …}`, is at most 65,536 bytes; a
larger one is replaced by the error `too_large`.

**Concurrency.** A task may make at most 256 tool calls; the next one fails the task with `limit`.
That call never runs. Whenever a task fails with `limit`, at this bound or at the durable-event
bound, every call of the task still open, this one included, is answered `task_ended` in the same
transaction, so the Agent is never left waiting for a tool result; a result that arrives later for
one of those calls is dropped. The instance is then sent `cancel` for the task, so a handler
waiting in `ask` returns. A tool call made after the task ended never runs: the instance that held
the task is answered `task_ended` at once, so it is not left waiting. Such a call counts toward the
task's 256 calls; past them it is ignored and never answered, so a cancelled handler should end
rather than call more tools. At most 8 calls of one task may be open at once; a call made while 8 are open is answered at
once with `concurrency_exceeded`. A result is delivered only while the task is live and its
instance still holds it. A call whose task ended before it ran is answered `task_ended`.

**Errors.** `tool_result.error` is `{code, message}`. `code` is a Services v1 status name (from the
target, from this Runtime's stream plane, or `task_ended`, `too_large`, `not_found`, `conflict`
from `artifact.publish`), or one of:

| Code | Meaning |
| --- | --- |
| `tool_denied` | the definition does not allow the tool's family |
| `invalid_tool` | an unknown tool, arguments that do not match it, a workspace path that escapes, a source that is not a regular file, a directory destination, or an unexpected answer |
| `too_large` | the result exceeds 64 KiB, or (from the SDK) the arguments exceed 16 KiB |
| `local_io` | a local file or caller-side I/O failure; local `not_found` and `permission_denied` keep those names |
| `concurrency_exceeded` | 8 calls of the task were already open |
| `outcome_unknown` | the effect of a re-executed mutation is unknown |
| `output_unavailable` | a re-executed `exec.run` was answered from replay: the command already ran, and the message names how it ended, but its output was not retained |

## Withdrawn authorization

Once a second, if the Store revision has changed, and every 15 s even without a change, every
unfinished task is checked. A task whose remote creator no longer has `Agent(agent)` (because of a
revoked grant, an `agent` unexposure, a departure, or a removal) is cancelled with reason
`unauthorized`, through [Cancellation](#cancellation): a queued task at once, a live one after the
Agent confirms or the 10 s grace period passes.

- Tasks the local Endpoint created depend on no grant and are never checked.
- A task whose cancellation was already requested is skipped.
- The reason is durable in the task row, so a Runtime restart keeps it.

## Retention and limits

| Bound | Value | Over the bound |
| --- | --- | --- |
| unfinished tasks per creator | 16 | `submit` is `reject(concurrency_exceeded)`, no effect |
| unfinished tasks per Runtime | 256 | `submit` is `reject(concurrency_exceeded)`, no effect |
| finished tasks retained per creator | 64 | the oldest finished tasks of that creator are removed when a new task is created |
| finished tasks retained per Runtime | 1,024 | the oldest finished tasks are removed when a new task is created |
| durable events per task | 256 | the task fails with `limit` |
| retained progress events per task | 64 | older progress events are pruned |
| tool calls per task | 256 | the task fails with `limit` |
| open tool calls per task | 8 | the call is answered `concurrency_exceeded` |
| tool name / arguments / result | 64 B / 16 KiB / 64 KiB | the line is ignored / the line is ignored / `too_large` |
| unacknowledged deliveries per instance | 256 | no new task is assigned to the instance |
| artifacts per task | 16 | `artifact.publish` answers `conflict` |
| artifact size | 256 MiB | `artifact.publish` answers `too_large` |

- **Expiry.** A finished task, with its events, artifact references, tool calls, and RequestId
  binding, is removed 7 days after `ended_ms`. The check runs every 60 s.
- **Blobs.** Blobs that no retained task references are then collected (see
  [Artifacts](#artifacts)).

| Timer | Value |
| --- | --- |
| `open` deadline | 15 s |
| assignment, instance supervision, cancellation grace check | every 1 s |
| convergence | after a durable change (checked every 1 s), and every 15 s |
| expiry and blob collection | every 60 s |
| cancellation grace | 10 s |
| remote `cancel` wait | 60 s |
| `ready` deadline | 30 s after the instance started |
| restart backoff | 1 s doubling to 60 s, reset after 60 s of uptime |
| stop grace | 5 s |
| unreferenced blob age | 10 minutes |

## Status codes

Agent adds five codes to the Services v1 status set:

| Code | Name | Meaning | Produced by |
| --- | --- | --- | --- |
| 96 | `unknown_agent` | not defined, or not available to this caller | every operation that names an Agent, after admission |
| 97 | `unknown_task` | not found, not created by the caller, or not this Agent's | `send`, `get`, `watch`, `cancel`, `artifact`; a `watch` whose task was removed |
| 98 | `agent_unavailable` | the Agent is stopped | `submit` |
| 99 | `task_ended` | the operation needs a live task | `send` (terminal, or cancellation requested); `artifact.publish`; a tool call whose task ended |
| 100 | `unknown_artifact` | the task has no artifact of that name | `artifact` |

Other codes used by Agent:

| Code | Used for |
| --- | --- |
| `invalid_input` | malformed bodies, a message with the wrong role or invalid data, an `offset` above the artifact size |
| `unauthorized` | coarse admission |
| `concurrency_exceeded` | stream limits and unfinished-task limits |
| `conflict` | a RequestId reused with a different `submit` body |
| `unavailable` | a Store failure, and a `watch` ended by shutdown or withdrawn authorization |
| `cancelled` | a `watch` the caller cancelled |
| `outcome_unknown` | a `cancel` whose task did not end within 60 s or before the operation stopped; caller-side lost streams |
| `precondition_failed`, `timed_out`, `internal` | the Transfer v1 download of `artifact` |

## Fault points (test-only)

Debug builds only. `MA2A_TEST_AGENT_FAULT=<point>` makes the daemon kill itself
(`ma2a_services::supervisor::crash()`: `SIGKILL` on Unix) at one named point, so tests can stop it
inside each crash window. Release builds compile the check out entirely.

| Point | Window |
| --- | --- |
| `after_task_created` | a new task is committed and not yet assigned |
| `after_assigned` | `running` and its delivery are committed, and the delivery is not yet written |
| `after_tool_recorded` | the tool call is pending and not yet executed |
| `before_tool_result` | the tool has executed and its result is not yet recorded |

## Known limitations

- **Not a sandbox.** An Agent program can use everything its OS user can, including the local API.
  The tool families restrict what the bridge does, not what the program does.
- **Proxy listeners are not durable.** A `proxy.open` listener lives in the daemon's memory. It
  closes when the daemon stops and is never reopened, so after a restart the recorded `listen`
  address is dead. While the daemon runs, a listener closes as soon as its task ends: at once on a
  cancellation, an Agent report, a lost instance, or the cancellation grace period, and within
  about 1 s on any other end, such as `agent_stop` or an event bound.
- **Instances are local Processes.** Each instance counts against the Process limits of the local
  Endpoint as creator (Process v1: 8 non-terminal and 16 non-archived records per creator), so at
  most 8 Agents run instances at once. Ended instance Processes, with their output, stay readable
  until a new instance needs the room: a start that finds 16 non-archived local records archives
  the local Process that ended first and tries once more.
- **Workspace downloads replace.** `transfer.get` and `agent.artifact` replace an existing
  workspace file of the same name.
- **Backoff and ready deadline.** The restart backoff is in memory and resets with the daemon. The
  `ready` deadline is measured on the wall clock from the instance's recorded start.
- **No cross-task context.** Each task is its own conversation; nothing groups tasks.
- **Compatibility.** Phase 3 Runtimes accept member capability bits `0xff` only. They cannot decode
  a signed Space member entry that carries `agent` (bit value `256`), so a Space owner should grant
  `agent` only once every member runs Phase 4.

## Deviations from the plan

The plan is `.omo/plans/ma2a-phase-4.md`. This document describes the code; it differs from the
plan as follows.

- **Message bound.** A message holds at most 12,288 bytes of part content, not 16 KiB encoded, so
  one fits a `submit` header with room to spare.
- **State events.** A `state` event carries no message. A message reported with a state is
  recorded as a separate `message` event just before it.
- **`artifact` body.** It carries an optional resume identity `[size, modified_ns]` besides
  `offset`.
- **Retained-task bounds.** Over 64 finished tasks per creator or 1,024 per Runtime, the oldest
  finished tasks are removed when a new task is created, instead of `submit` being refused. Only the
  unfinished-task bounds refuse.
- **Remote `cancel`.** It waits at most 60 s for the end, then answers `outcome_unknown`.
- **Local `agent_task_cancel`.** It returns the task at once, once the cancellation is recorded,
  and does not wait for the task to end; neither does the local `ma2a agent cancel`. `ma2a agent
  watch` follows the task to its end.
- **Pending cancellation wins.** A task whose cancellation was requested ends `cancelled` with the
  requested reason when its instance ends or its Agent is stopped, not `failed(agent_lost)` or
  `failed(agent_stopped)`.
- **Tool family check.** A call is recorded first and its family is checked when it executes, so a
  denied call is recorded and counts toward the 256-call bound.
- **Open tool calls.** A call made while 8 calls of its task are open is answered at once with
  `concurrency_exceeded`; it is not queued.
- **Derived RequestIds.** Only calls that open a mutation use the derived RequestId. Reads and each
  proxy connection use a fresh random one.
- **Proxy listeners** end with the daemon and are not reopened after a restart; a listener of a
  task that ended without a cancellation, report, or lost instance closes within about 1 s rather
  than at once (see [Known limitations](#known-limitations)).
- **Tool additions.** `files.list` takes `after` for paging. `exec.run` takes `timeout_ms`
  (default 60 s) and reports `reason` and `truncated`; its capture is 64 KiB per stream, cut to fit
  the 64 KiB result bound, instead of the planned 1 MiB.
  `process.terminate` defaults `grace_ms` to 5,000.
- **Tool error names.** Besides the Services v1 names, `tool_denied`, `invalid_tool`, and
  `outcome_unknown`, tools also report `local_io`.
- **Protocol violations** are ignored and only advance the cursor; they are not counted. A line
  that would break a Store bound is treated as one.
- **Delivery framing.** A Runtime line is written as `data` frames of at most 64 KiB each, not as
  one frame, and may be up to 16 MiB.
- **Artifact output.** `ma2a agent artifact --output FILE` never overwrites an existing file. The
  `agent.artifact` tool, by contrast, replaces an existing workspace file.
- **Local artifact reads** are chunked through the local API (`agent_task_artifact_read`, at most
  32 KiB per response, base64) instead of a stream.
- **Blob collection** spares unreferenced blobs younger than 10 minutes.
- **Definitions.** Environment names starting with `MA2A_AGENT_` are refused. `max_tasks` has no
  protocol default; the CLI defaults it to 1, not 4.
- **Reference Agent.** `ma2a-example-agent` also has `progress`, `fail`, `crash`, and `tool`
  commands.
