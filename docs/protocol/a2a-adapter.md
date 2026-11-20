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
- **Listener.** `--listen` must be a loopback address; any other address is a usage error. The default is `127.0.0.1:41241`, and port `0` picks a free port.
- **Token.** Every route requires `Authorization: Bearer TOKEN`, including the Agent Cards.
  - The token is 32 random bytes as 64 lowercase hex characters, new for every run.
  - It is written to `STATE_DIR/a2a-token`, or to `--token-file`. The adapter first creates a fresh directory `.ma2a-a2a-stage-HEX` beside it (32 random hex characters; a name already taken is left alone and another tried), owner-only on Unix and, on Windows, with a protected ACL for the current user and SYSTEM that its children inherit. The token is written to a file inside, which is made private and checked (mode `0600` and owned by the current user on Unix; on Windows a protected ACL admitting only the current user and SYSTEM, on an object they or Administrators own), then renamed into place, and the staging directory is removed. So the file is private from the moment it exists, and nothing a shared parent directory passes on ever reaches it. On failure the adapter removes only what it created and stops. Within the moment between creating the staging directory and protecting it, the directory is empty; an account the parent lets in sees only its name.
  - One adapter at a time serves a given token file. The adapter holds an OS lock on `.ma2a-a2a-locks/NAME`, beside the token file `NAME`, from before it writes the token until it exits. The lock carries the token file's own name in the same directory, so the file system decides which spellings name one file, and every Runtime of this account contends for it whatever its state directory. A second adapter naming the same file, by any spelling of its path (the default `STATE_DIR/a2a-token` included), stops with a usage error before it writes anything. `.ma2a-a2a-locks` must be private to this account (owner-only on Unix, the protected ACL above on Windows); one that is not is refused and left as it was. A `--token-file` named with the `.ma2a-a2a-` prefix, in any ASCII letter case, or inside such a directory, is refused. Give each concurrent adapter its own `--token-file`. The empty lock file stays behind. Adapters built before this layout lock elsewhere; stop them before starting a newer one on the same token file.
  - It is never printed or logged. When the adapter stops (Ctrl-C or `SIGTERM`) the file is removed if it still holds this run's token, so a second adapter that wrote the same file since keeps its own. A stop lets open requests and streams finish for at most 5 s, then closes them.
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

A card is served for a defined local Agent, found through `agent_list`, even while it is stopped. For a remote Agent, the card is served only if Agent v1 `agents` lists it for this Endpoint. Otherwise the card route answers `404`.

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
  - A wrong envelope, or a missing or non-scalar `id`, answers `-32600`.
  - An unknown method answers `-32601`.
- Every answer, error or not, is HTTP `200` with `application/json`, except for the streaming methods.
- Each error carries one `google.rpc.ErrorInfo` in `data`, with the A2A `reason` and domain `a2a-protocol.org`.

## Methods

| Method | Behaviour |
| --- | --- |
| `SendMessage` | Creates a task, or continues the task the message names. By default it waits until the task ended or waits for input, then answers `{task}`. `configuration.returnImmediately: true` answers at once. `configuration.historyLength` limits the history returned. |
| `SendStreamingMessage` | Same delivery as `SendMessage`, answered as Server-Sent Events: first `{task}`, then one `statusUpdate` or `artifactUpdate` per new event. The stream closes once the task ended or waits for input. A message that continues a waiting task is followed from the moment it was sent, so the stream closes only once the task waits again or ends. |
| `GetTask` | `{id, historyLength?}`: the task with its artifacts and history. |
| `ListTasks` | The route's tasks visible to the caller. See [ListTasks](#listtasks). |
| `CancelTask` | `{id}`: requests cancellation, waits up to 30 s for the task to end, then answers the task as it is. |
| `SubscribeToTask` | `{id}`: Server-Sent Events like a stream, starting with `{task}` and closing once the task ended. A task that has already ended answers `-32004`. |
| `*PushNotificationConfig` | `-32003 PushNotificationNotSupported`. |
| `GetExtendedAgentCard` | `-32007 ExtendedAgentCardNotConfigured`. |

Each Server-Sent Event's `data:` is one complete JSON-RPC response with the request's `id`. A refusal after the stream started is sent as a final error response.

## Mapping

### Messages and idempotency

- A client message must have `role: "ROLE_USER"`, a non-empty `messageId`, and 1–16 parts within the Agent v1 message bound.
  - A `text` part maps to a text part.
  - A `data` part maps to a data part that carries its JSON.
  - A `raw` or `url` (file) part answers `-32005 ContentTypeNotSupported`.
- The `messageId` names the operation. Its `RequestId` is:

  ```text
  blake3::derive_key("ma2a a2a-adapter v1 message request id", route_path ‖ 0x00 ‖ messageId)[..16]
  ```

  `route_path` is the route's path, for example `/agents/lead` or `/endpoints/{endpoint}/agents/worker`.
  - Resending the same message to the same route replays the original operation and answers the same task.
  - Reusing a `messageId` for a different message conflicts, and answers `-32603` with reason `CONFLICT`.
  - The same `messageId` sent to two routes names two operations.
- `taskId` or `contextId` names the task a message continues; when both are present they must be equal. A message without either creates a task.

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

- The adapter follows events from the task's newest sequence number, so no event is reported twice.
- A follow interrupted by a transient remote failure reconnects from the last delivered event.

### Artifact content

`GET …/tasks/{task}/artifacts/{name}` streams the content with the artifact's media type. It never delivers a complete body that does not match the artifact's BLAKE3.

- **Local artifact.**
  - The body carries `Content-Length`.
  - It is read in local API chunks of at most 32 KiB.
  - The last chunk is sent only after the whole content verified. A mismatch ends the body early.
- **Remote artifact.**
  - The body is chunked, with no length.
  - It is read through Agent v1 `artifact` (Transfer v1 download frames).
  - The body ends with an error unless the download verified.
- An unknown task or artifact answers `404`.

### ListTasks

`ListTasks` answers `{tasks, nextPageToken, pageSize, totalSize}`.

- **Source.** It reads every page of the route's tasks: `agent_task_list` locally, Agent v1 `tasks` remotely.
- **Filters**, all optional:
  - `contextId`: the task;
  - `status`: an A2A `TaskState`;
  - `statusTimestampAfter`: ISO 8601 UTC.
- **Paging.**
  - `pageSize` is 1–100, default 50.
  - `pageToken` is the last task ID of the previous page. Tasks are listed in task ID order, so the next page starts after that ID even if that task no longer matches. A token that is not a task ID answers `-32602`.
  - `totalSize` counts every match.
- **Detail.** Artifacts are included only with `includeArtifacts: true`. History is included only when `historyLength` is greater than 0. Without either, no task's details are read.

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
| `-32004` `UnsupportedOperation` | A message to, or a subscription on, a task that has already ended. |
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
