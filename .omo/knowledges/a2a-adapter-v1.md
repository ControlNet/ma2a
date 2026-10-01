# A2A v1.0 adapter: facts and pitfalls (Phase 4)

Spec facts verified against `a2a.proto` and the v1.0 spec, and used by `crates/ma2a-app/src/a2a/`:

- **Methods.** JSON-RPC method names are PascalCase: `SendMessage`, `SendStreamingMessage`, `GetTask`, `ListTasks`, `CancelTask`, `SubscribeToTask`, `*TaskPushNotificationConfig`, `GetExtendedAgentCard`.
- **Enums** are proto-style: `ROLE_USER` / `ROLE_AGENT` and `TASK_STATE_*`. Note the A2A spelling `TASK_STATE_CANCELED`, with one L; the local state is `cancelled`.
- **Part** is a oneof of `text`, `raw`, `url` and `data`, plus `mediaType` and `filename`. There is no `kind` discriminator in v1.0.
- **`StreamResponse`** is exactly one of `task`, `message`, `statusUpdate` or `artifactUpdate`. Each SSE `data:` carries a full JSON-RPC response with the request's `id`.
- **Versioning.** `A2A-Version` comes as a header or a query parameter; when absent, it means 0.3. Answer `-32009` for anything but `1.0`.
- **Error codes:**
  - `-32001` TaskNotFound
  - `-32002` NotCancelable
  - `-32003` PushNotSupported
  - `-32004` Unsupported
  - `-32005` ContentType
  - `-32007` ExtendedCardNotConfigured
  - `-32009` VersionNotSupported
- **Card path.** The well-known card is `/.well-known/agent-card.json`, relative to each Agent route.

Pitfalls hit while building it:

- **A message to a waiting task.** The send answers a summary that is still `waiting`. Waiting for a "settled" state would return immediately, with no new Agent message. Wait only for a state reached *after* the send (`view::wait` with `fresh = true`).
- **Local task history.** The local event log starts after the input, so the input must be prepended. Remote `get` details carry only input and result, so remote history is just those two messages (documented mismatch).
- **Two meanings of `conflict`.** The local API answers both "messageId reused with a different body" and "message to an ended task" as `conflict`. The adapter tells them apart by reading the task's state, so an ended task maps to `-32004` on both local and remote routes.
- **Stream end.** When the follow future ends, drain the event channel with `try_recv` before closing. Otherwise the last state update can be lost: the follower stops once it sees the terminal state, but the consumer may not have read it yet.
- **Follow loops must stop when the client goes away.** Select on `events.closed()` (remote) or check `events.is_closed()` (local polling). Otherwise an abandoned SSE keeps a stream-plane watch open.
- **Remote artifact bodies have no `Content-Length`.** The body is wrapped in a stream that yields an error at its end unless Transfer v1 verified the BLAKE3, so a client never sees a complete wrong body. Local artifacts withhold the last chunk until the whole file verified.
- **Tests that spawn the adapter.** Use `Stdio::null()` for its stderr. An inherited, piped stderr kept the test harness's output pipe open, and the pipeline hung after the test ended. Also kill the adapter in `Drop`, so a failing assertion does not leak it.
- **Leaked Agent instances.** Agent instances are Phase 3 Processes under supervisors that outlive the daemon. The E2E lab therefore stops its Agents in a drop guard declared before the daemons. Check for leftovers with:

  ```bash
  pgrep -fa "ma2a-example-agent|a2a serve"
  ```

- **Unix socket path limit (103 bytes).** Keep E2E labels short, or supervisor sockets fail with `spawn_failed`.
