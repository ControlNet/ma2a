# A2A v1.0 Adapter

`ma2a a2a serve` lets A2A clients use this Runtime's Agents, and other Endpoints' Agents through this Runtime. It is an A2A v1.0 server using the JSON-RPC 2.0 binding over HTTP. Agent v1 (`agent-v1.md`) is the native protocol, and this adapter translates A2A onto it. It adds no authority of its own.

```text
ma2a a2a serve [--listen 127.0.0.1:41241] [--token-file PATH]
```

## Trust model

- The adapter is a separate process. It is a client of the local Runtime API, running as the same user, and it requires a running daemon.
- A2A callers have no Endpoint identity.
  - For this Runtime's own Agents, the adapter acts as the local operator.
  - For another Endpoint's Agents, it acts as this Endpoint. Every remote operation goes through the stream plane under this Endpoint's identity, so it needs one complete Space path to that Agent, exactly as the CLI does.
- **Remote answers.** A remote `submit` must be accepted as Submitted, and any other Agent v1 operation as Admitted. The Summary a `submit` answers must name the RequestId the adapter sent, so another submission's task is never taken as this one. A `submit`, `send`, or `cancel` may also be answered from the caller's replay table with no accept. Any other answer fails the call: a method answers `-32603` with reason `INTERNAL`, and the card route answers `404`.
- **Remote deadline.** A remote Agent v1 `agents`, `submit`, `send`, `get`, or `tasks` that the target does not answer within 30 s fails, and the method answers `-32603` with reason `TIMED_OUT`. A remote `cancel` is bounded by the wait of `CancelTask` instead, and a stream or an artifact download lasts while its client stays connected.
- **Listener.** `--listen` must be a loopback address; any other address is a usage error. The default is `127.0.0.1:41241`, and port `0` picks a free port.
- **Connections.** At most 64 connections are served at once. A request has 10 seconds to send its headers and, once authenticated, 10 more for its body (`408` otherwise).
- **Token.** Every route requires `Authorization: Bearer TOKEN`, including the Agent Cards.
  - The token is 32 random bytes as 64 lowercase hex characters, new for every run.
  - It is written to `STATE_DIR/a2a-token`, or to `--token-file`. The adapter first creates a fresh directory `.ma2a-a2a-stage-HEX` beside it (32 random hex characters; a name already taken is left alone and another tried), owner-only on Unix. On Windows it is created with a protected ACL already in place, granting only its owner and SYSTEM and inherited by its children, and then given the exact current-user-and-SYSTEM ACL. The token is written to a file inside, which is made private and checked (mode `0600` and owned by the current user on Unix; on Windows a protected ACL admitting only the current user and SYSTEM, on an object they or Administrators own), then renamed into place, and the staging directory is removed. So the file is private from the moment it exists, and nothing a shared parent directory passes on ever reaches it. On failure the adapter removes only what it created and stops. No other account is ever granted access to the staging directory or the file in it, so none can hold a handle to either.
  - One adapter at a time serves a given token file. The adapter holds an OS lock on `.ma2a-a2a-locks/NAME`, beside the token file `NAME`, from before it writes the token until it exits. The lock carries the token file's own name in the same directory, so the file system decides which spellings name one file, and every Runtime of this account contends for it whatever its state directory. A second adapter naming the same file, by any spelling of its path (the default `STATE_DIR/a2a-token` included), stops with a usage error before it writes anything. `.ma2a-a2a-locks` must be private to this account (owner-only on Unix, the protected ACL above on Windows); one that is not is refused and left as it was. A `--token-file` named with the `.ma2a-a2a-` prefix, in any ASCII letter case, or inside such a directory, is refused. The token file's directory is resolved once, before the lock is taken, and the lock, the staging directory, the token, and its removal all use that resolved directory, so a link on the given path pointed elsewhere meanwhile moves none of them. Give each concurrent adapter its own `--token-file`. The empty lock file stays behind. Adapters built before this layout lock elsewhere; stop them before starting a newer one on the same token file.
  - It is never printed or logged. When the adapter stops (Ctrl-C or `SIGTERM`), or fails after writing it, even before printing its ready line, the file is removed if it is still a regular file (not a link) holding this run's token, so a second adapter that wrote the same file since keeps its own. A stop closes idle connections at once and serves no new request; open requests and streams get at most 5 s to finish, then are closed.
  - It is compared in constant time. A missing or wrong token answers `401` with `WWW-Authenticate: Bearer`.
- **Ready line.** Once listening, the adapter prints one JSON line to stdout and nothing else:

  ```json
  {"url": "http://127.0.0.1:41241", "token_file": "/path/to/a2a-token"}
  ```

  A client reads the token from that file. Do not commit the file or copy it into shared logs.

## Routes

| Route | Method | Purpose |
| --- | --- | --- |
| `/agents/{agent}` | `POST` | JSON-RPC for this Runtime's Agent `agent` |
| `/endpoints/{endpoint}/agents/{agent}` | `POST` | JSON-RPC for Agent `agent` on Endpoint `endpoint` (64 lowercase hex characters) |
| `…/.well-known/agent-card.json` | `GET` | The Agent Card of the route |
| `…/tasks/{task}/artifacts/{name}` | `GET` | The content of one artifact; `name` is percent-encoded |

- A malformed `agent` or `endpoint` segment answers `404`.
- The JSON-RPC routes answer a call to an Agent the caller cannot use with an error, described below.
- Request bodies are limited to 64 KiB.

## Agent Card

A card is served for a defined local Agent, found through `agent_list`, even while it is stopped. For a remote Agent, the card is served only if Agent v1 `agents` lists it for this Endpoint. The adapter reads at most 64 pages of that list. A page must list Agents in name order after the cursor sent, and a next cursor must be the page's last Agent; the adapter stops at any other page, or once 64 pages still name a next cursor. Otherwise the card route answers `404`. If the Runtime or the remote target cannot answer now (unavailable, unreachable, busy, timed out, or its answer was lost), the card route answers `503` instead, so a client can retry.

| Field | Value |
| --- | --- |
| `name` | The `AgentId` |
| `description` | The Agent's description, or `MA2A Agent {name}` when it is empty |
| `supportedInterfaces` | One entry: the route URL, `protocolBinding: "JSONRPC"`, `protocolVersion: "1.0"` |
| `version` | The `ma2a` version |
| `capabilities` | `streaming: true`, `pushNotifications: false`, `extendedAgentCard: false` |
| `securitySchemes`, `securityRequirements` | One HTTP `Bearer` scheme, required |
| `defaultInputModes` | `text/plain`, `application/json` |
| `defaultOutputModes` | `text/plain`, `application/json`, `application/octet-stream` |
| `skills` | One skill named after the Agent, tagged `ma2a` |

## Versioning

- Every JSON-RPC call must carry `A2A-Version: 1.0`, either as a header or as an `A2A-Version` query parameter.
- A missing version means 0.3, which this adapter does not speak. A missing or different version answers `-32009 VersionNotSupported`.

## JSON-RPC envelope

- A request is `{"jsonrpc": "2.0", "id": STRING|NUMBER, "method": M, "params": {…}}`.
  - Unparseable JSON answers `-32700`.
  - A wrong envelope, a missing or non-scalar `id`, or `params` that is neither an object nor absent answers `-32600`. The error carries the request's `id` when it is a string or a number, and `null` otherwise.
  - An unknown method answers `-32601`.
- Every answer, error or not, is HTTP `200` with `application/json`, except for the streaming methods.
- Each error carries one `google.rpc.ErrorInfo` in `data`, with the A2A `reason` and domain `a2a-protocol.org`.

## Methods

| Method | Behaviour |
| --- | --- |
| `SendMessage` | Creates a task, or continues the task the message names. By default it waits until the task ended or waits for input, then answers `{task}`. `configuration.returnImmediately: true` answers at once. `configuration.historyLength` limits the history returned. Every parameter is checked before the message is delivered: a `configuration` that is not an object, a `returnImmediately` that is not a boolean, or a negative `historyLength` answers `-32602` and delivers nothing. |
| `SendStreamingMessage` | Same delivery and parameter checks as `SendMessage`, answered as Server-Sent Events: first `{task}`, with at most `configuration.historyLength` history messages, then one `statusUpdate` or `artifactUpdate` per new event. The stream closes once the task ended or waits for input. A message that continues a waiting task is followed from the moment it was sent, so the stream closes only once the task waits again or ends. Events the first `{task}` already shows are not sent again, and a first `{task}` that has ended closes the stream. |
| `GetTask` | `{id, historyLength?}`: the task with its artifacts and history. |
| `ListTasks` | The route's tasks visible to the caller. See [ListTasks](#listtasks). |
| `CancelTask` | `{id, historyLength?}`: requests cancellation, waits up to 30 s for the task to end, the request to a remote target included, then answers the task as it is. |
| `SubscribeToTask` | `{id}`: Server-Sent Events like a stream, starting with `{task}` and closing once the task ended. A task that has already ended answers `-32004`. |
| `*PushNotificationConfig` | `-32003 PushNotificationNotSupported`. |
| `GetExtendedAgentCard` | `-32007 ExtendedAgentCardNotConfigured`. |

Each Server-Sent Event's `data:` is one complete JSON-RPC response with the request's `id`. A refusal after the stream started is sent as a final error response.

## Mapping

### Messages and idempotency

- A client message must have `role: "ROLE_USER"`, a non-empty `messageId`, and 1–16 parts within the Agent v1 message bound.
  - Each part carries exactly one of `text`, `data`, `raw`, or `url`; a null field counts as absent. A part with none or several of them, or whose `text`, `raw`, or `url` is not a string, answers `-32602` before anything is delivered. Other fields, such as `mediaType`, are ignored.
  - A `text` part maps to a text part.
  - A `data` part maps to a data part that carries its JSON.
  - A `raw` or `url` (file) part answers `-32005 ContentTypeNotSupported`.
- The `messageId` names the operation. Its `RequestId` is:

  ```text
  blake3::derive_key("ma2a a2a-adapter v1 message request id", route_path ‖ 0x00 ‖ messageId)[..16]
  ```

  `route_path` is the route's path, for example `/agents/lead` or `/endpoints/{endpoint}/agents/worker`.
  - Resending the same message to the same route replays the original operation and answers the same task.
  - Reusing a `messageId` for a different message conflicts, and answers `-32603` with reason `CONFLICT`, even once the task has ended.
  - The same `messageId` sent to two routes names two operations.
- `taskId` or `contextId` names the task a message continues; when both are present they must be equal. A message without either, or with them null or empty, creates a task. Any value other than a string answers `-32602`.

### Tasks

| Local state | A2A `TaskState` |
| --- | --- |
| `queued` | `TASK_STATE_SUBMITTED` |
| `running` | `TASK_STATE_WORKING` |
| `waiting` | `TASK_STATE_INPUT_REQUIRED` |
| `completed` | `TASK_STATE_COMPLETED` |
| `failed` | `TASK_STATE_FAILED` |
| `cancelled` | `TASK_STATE_CANCELED` |

- **Identity.** `id` is the 32-hex `AgentTaskId`, and `contextId` is always the same value.
- **`status.timestamp`** is when the task ended, started, or was created, whichever is latest, in ISO 8601 UTC.
- **`status.message`** depends on the state:
  - completed: the result;
  - failed: the error as an agent text message;
  - input required: the newest Agent message.
- **`history`**, oldest first, newest kept when `historyLength` limits it:
  - local task: the input, then every retained message event;
  - remote task: the input and the result only, because Agent v1 `get` carries no message events.
- **`artifacts`.** Each artifact has `artifactId` and `name` equal to its artifact name, and one file part. That part's `url` is the adapter's artifact route, with `filename` and `mediaType`. `metadata.size` and `metadata.blake3` identify the content.
- **`metadata.ma2a`** carries `agent`, `creatorEndpointId`, `fromAgent` (attribution only), `reason`, and `progress`.

### Stream updates

| Agent v1 event | Update |
| --- | --- |
| `state` | `statusUpdate` with the new state |
| `message` from the Agent | `statusUpdate` with the current state and that message |
| `message` from the creator | none, because it is already in the history |
| `progress` | `statusUpdate` with the current state and the progress text as an agent message |
| `artifact` | `artifactUpdate` with the artifact, `append: false`, `lastChunk: true` |

- The first `{task}`, the state the stream starts from, and the sequence number it follows from all come from one read of the task; a message that continues a waiting task is followed from the moment it was sent instead. A local task's history holds only the events that read counted, so no event is reported twice.
- A follow interrupted by a transient remote failure reconnects from the last delivered event.
- A client that disconnects ends the stream at once, even while the first read of the task is still waiting for the target.

### Artifact content

`GET …/tasks/{task}/artifacts/{name}` streams the content with the artifact's media type. It never delivers a complete body that does not match the artifact's BLAKE3.

- **Local artifact.**
  - The body carries `Content-Length`.
  - It is read in local API chunks of at most 32 KiB.
  - The last chunk is sent only after the whole content verified. A mismatch ends the body early.
- **Remote artifact.**
  - The body is chunked, with no length.
  - It is read through Agent v1 `artifact` (Transfer v1 download frames).
  - The body ends with an error unless the download verified, and unless its size and BLAKE3 are the ones the task's artifact reference names. A download of another size is refused before any byte is sent.
  - A client that disconnects ends the download at once, even while the target sends nothing.
- An unknown task or artifact answers `404`, as does a task this caller may not see.
- A Runtime or remote target that cannot answer now, because it is unavailable, unreachable, busy, timed out, or its answer was lost, answers `503`, so a client can retry.

### ListTasks

`ListTasks` answers `{tasks, nextPageToken, pageSize, totalSize}`.

- **Source.** It reads every page of the route's tasks: `agent_task_list` locally, Agent v1 `tasks` remotely.
  It stops after 41 pages, enough for every task a Runtime retains. A remote page must list only tasks of the route's Agent, with task IDs in ascending order after the cursor sent, and a next cursor must be the page's last ID; any other page fails the call, so a target cannot keep the listing going, hide a task from the page tokens, or pass another Agent's task off as this one's.
- **Filters**, all optional:
  - `contextId`: the task;
  - `status`: an A2A `TaskState`;
  - `statusTimestampAfter`: ISO 8601 UTC; keeps tasks whose status last changed at or after it. Tasks are timed to the millisecond, so a finer timestamp counts from the next millisecond.
  - Absent, null, or empty filters nothing. A filter or `pageToken` that is not a string answers `-32602`, and every parameter is checked before any task is read.
- **Paging.**
  - `pageSize` is 1–100, default 50.
  - `pageToken` is the last task ID of the previous page. Tasks are listed in task ID order, so the next page starts after that ID even if that task no longer matches. A token that is not a task ID in its listed lowercase form answers `-32602`.
  - `totalSize` counts every match.
- **Detail.** Artifacts are included only with `includeArtifacts: true`; an `includeArtifacts` that is not a boolean answers `-32602`. History is included only when `historyLength` is greater than 0. Without either, no task's details are read. With artifacts but no history, a local task's events are read only while it waits for input, because its status message is the newest Agent message among them.

## Errors

| Code | When |
| --- | --- |
| `-32700` | The body is not JSON. |
| `-32600` | The JSON-RPC envelope is wrong. |
| `-32601` | Unknown method. |
| `-32602` | Invalid parameters, or the Runtime refused a parameter (`invalid_input`). |
| `-32001` `TaskNotFound` | No such task for this caller and Agent, including a message whose `taskId` names none. |
| `-32002` `TaskNotCancelable` | `CancelTask` on a task that has already ended. |
| `-32003` | Push notification methods. |
| `-32004` `UnsupportedOperation` | A message to a task that has already ended or is being cancelled, or a subscription on a task that has already ended. |
| `-32005` | A file part in a request. |
| `-32007` | `GetExtendedAgentCard`. |
| `-32009` | Missing or unsupported `A2A-Version`. |
| `-32603` | Any other refusal. Its `ErrorInfo.reason` is the Runtime's error name in upper case, for example `UNKNOWN_AGENT`, `UNAUTHORIZED`, `AGENT_UNAVAILABLE` (the Agent is stopped), `CONCURRENCY_EXCEEDED`, `CONFLICT` (a `messageId` reused for another message), or `UNAVAILABLE`. |

## Mismatches with A2A v1.0

These are documented, not emulated.

- **Contexts.** A context is a task: `contextId` always equals the task `id`, and a context never groups several tasks.
- **Push notifications** are not supported.
- **File parts in requests** are not accepted. Larger inputs belong in a Files root that the Agent reads with its tools.
- **States.** `TASK_STATE_REJECTED` and `TASK_STATE_AUTH_REQUIRED` are never produced. A refused message is a JSON-RPC error, not a task.
- **Authentication.** The adapter listens on loopback only and requires its bearer token on every route, including Agent Cards, which A2A otherwise serves publicly.
- **Remote history** is the input and the result only.
- **ListTasks history** is included only when `historyLength` is greater than 0, so a bare listing never reads every task's details.
- **Idempotency.** The `messageId` idempotency is per route, because the RequestId includes the route path.
- **Cancellation** waits at most 30 s. A task that has not ended by then is answered in its current state, and it still ends: by the Agent, or by the 10 s grace period of Agent v1.
