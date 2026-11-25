# Hosted Service v1

Phase 5 lets a third-party program serve a **hosted service** through its Runtime, and lets any
Endpoint discover the services another Endpoint serves it:

```text
third-party service process
↔ local MA2A Runtime IPC (same OS user)
↔ authenticated remote MA2A peer
```

The Runtime keeps network identity, remote authentication, Space authorization, resource
admission, and protocol framing. A hosted service never touches the network and never sees a
Space: it only receives calls that its Runtime has already authorized, together with the
authenticated caller Endpoint and the caller's RequestId.

Every service, built-in or hosted, runs through one pipeline: coarse admission, the bounded
`open`, target and family checks, fine authorization, the withdrawal watch, the operation
registry, dispatch, and the content-free report. Hosted services reuse the Services v1 frame
layer, status envelope, and caller-side stream plane unchanged (`docs/protocol/services-v1.md`).
The local operator commands and the link protocol are in `docs/protocol/local-api-v1.md`
("Hosted services"). The Rust SDK is the `ma2a-service` crate, and `ma2a-example-service` is a
deterministic example built on it, not a production service.

There is no global registry, no in-process plugin, and no schema or code generation.

## Model

| Type | Definition |
| --- | --- |
| `ServiceName` | 1–63 bytes of `[a-z0-9-]`, starting with a letter, not ending with `-`. The built-in family names (`echo`, `files`, `transfer`, `exec`, `shell`, `proxy`, `process`, `agent`, `service`) are reserved. |
| version | a protocol major version, `u16` 1–65,535 |
| `OperationName` | 1–32 bytes of `[a-z0-9_.-]`, starting with a letter |
| interaction | `unary` 0, `stream` 1, `bytes` 2, `resource` 3: what an operation's frames look like. It is descriptive; the Runtime enforces the Services v1 frame rules for all of them. |
| manifest | a description of at most 256 bytes; 1–8 versions in strictly ascending order, each with 1–16 distinct operations and at most 64 operations in all; `max_calls` 1–64. Encoded, at most 4 KiB. |
| descriptor | what discovery returns: the name, built-in or hosted, the description, the versions with their operations, and `available`. Never a Space, a member, or an authorization path. |

Every built-in family is described by the catalog (`ma2a_core::service::catalog`): version 1, its
operations with their interactions, its coarse admission, and its stream bounds. A test asserts
that every operation of every typed body is in its family's descriptor.

## Transport

The ALPN is exactly `ma2a/service/1`, and the service code in the `open` header is `9`
(`ServiceKind::Service`). It is negotiated like the other service ALPNs.

- **Streams.** One operation uses one QUIC bidirectional stream.
- **Concurrency.** At most 16 streams per peer and 64 Runtime-wide; a stream over either bound
  is `concurrency_exceeded` before any byte is read. Each hosted service also admits at most its
  `max_calls` running calls.

## Authorization

- **`describe`** needs `ServiceAccess::Discover`: the caller and this Runtime are current members
  of one Space. It is the coarse admission of the family.
- **`call`** needs `ServiceAccess::Hosted(name)`: **one single Space** in which both are current
  members, the caller's signed member grant holds `service` (capability bit value `512`), and this
  Runtime exposes `name` to that Space (exposure kind `6` `service`, selector `name`).

A name that is not registered, not exposed to the caller, or not granted answers
`reject(unknown_service)`, identically. Grants and exposures from different Spaces never combine,
and the authorizing Space is never named or revealed. A withdrawn authorization cancels a running
call (a relayed `cancel` and the 5 s cancel grace), and its operation ends `unauthorized`.

## Open bodies

The body (key `4`) of the `open` header is a tagged canonical CBOR array:

| Tag | Operation | Body | Class |
| --- | --- | --- | --- |
| 0 | `describe` | `[0, after: tstr / null]` | read |
| 1 | `call` | `[1, service: tstr, version: uint, operation: tstr, body: bstr]` | mutation |

A `call` body is at most 8,192 opaque bytes. Every `call` is a mutation, so it uses caller-side
durable replay like Exec: the same RequestId with the same call returns the recorded terminal
without calling the service again, and the same RequestId with a different call is `conflict`.
The service receives the caller Endpoint and the RequestId with every call, so it can make its own
effects idempotent.

## Discovery

`describe` answers with one outcome only (no `accept`). Its result is `[descriptors, next]`, with
at most 16 descriptors and `next` the name to continue after, or `null`. A descriptor is
`[origin: 0 built-in / 1 hosted, name, description, versions, available: bool]`; a version is
`[version, [[operation, interaction]…]]`.

The caller library waits at most 30 seconds for that answer, dial included, and treats any other
frame as a protocol error.

For this caller only, discovery lists:

1. every built-in family whose coarse admission the caller passes (Echo through its own policy
   check), in catalog order, with `available` true;
2. then every registered hosted service the caller may `call`, by name, with `available` true
   only while the service is `ready`.

Because built-in names are reserved, a name alone says which of the two it is, and a page's
`after` cursor works across both.

A service is left out only when the caller is not authorized for it. If the target cannot decide,
for example because its store is unavailable, the whole page fails with that status, so the caller
never mistakes a service it may use for one that is not there.

## Versioning

- The `open` envelope stays exactly `[1, 0]`; any other value is `version_mismatch`.
- Every built-in family speaks version 1, and its descriptor says so.
- A hosted service declares its versions in its manifest. A `call` names one exact version: a
  version the manifest does not list is `reject(version_mismatch)`, and an operation that version
  does not list is `reject(invalid_input)`. The Runtime never negotiates.
- Selection is the caller's and deterministic: the highest version that both the caller supports
  and the descriptor lists (`ma2a_service::select_version`). `ma2a service call` without
  `--version` picks the highest version listing the operation, paging discovery at most 8 times
  and refusing a page whose services or `next` do not come after its `after` cursor as
  `invalid_input`. The chosen version is visible to
  the caller, to the service on every call, and in the per-version call counters of
  `service_hosted_list`.

A service evolves by adding a version and keeping the old ones while callers still use them.
`ma2a-example-service` shows this: version 2 adds `count`, `hold`, and `pipe` to version 1's
`echo` and `upper`, so a caller of version 1 keeps working unchanged.

## Calls

After `open`, a call carries whatever the service sends, relayed verbatim and checked against the
Services v1 direction rules:

| Frame | Payload |
| --- | --- |
| `accept` | a CBOR byte string of at most 8 KiB: the service's opaque header. Only after it may the call have effects. |
| `data`, `end` | output on channel 1 and errors on channel 2 from the service; input on channel 0 from the caller |
| `control` | opaque, at most 16 KiB, in both directions |
| `reject` | the call had no effect |
| `outcome` | `{0: status, 1: result / null}`: on success a CBOR byte string of at most 60,000 bytes |

A unary operation may answer with an `outcome` without accepting first. A caller `cancel`, or the
caller leaving, reaches the service as `cancel`; the service should end the call with
`outcome(cancelled)` after undoing what it can.

## Hosting and lifecycle

Registrations are durable (Store schema 10, table `hosted_services`); links and calls are not.

| State | Meaning |
| --- | --- |
| `stopped` | disabled; calls are `reject(unavailable)` |
| `unavailable` | started, without a process that reported ready; calls are `reject(unavailable)` |
| `ready` | a process is attached and ready: the only state that takes calls and is advertised as available |

**Link.** A service process attaches with `service_attach` and reports `{"t":"ready"}` on its link.
A service has at most one link. The link's end makes the service `unavailable` at once; there is
no heartbeat, because the OS closes the socket of a dead process.

**Call hand-over.** The Runtime admits a call within `max_calls` (otherwise
`reject(concurrency_exceeded)`) and sends the link only the call's ID. The process accepts it on a
fresh connection with `service_accept`, and the answer `service_call` is the first time it learns
the caller, RequestId, version, operation, and body. The hand-over is atomic, so a call is either
delivered or withdrawn, never both: one not accepted within 10 s, or still waiting when the
service stops, is withdrawn and answered `reject(unavailable)` with no possible effect. A delivered
call whose service connection is lost ends with `outcome(outcome_unknown)`.

**Stop.** `service_stop` disables the service, sends `{"t":"stop"}` on the link, withdraws calls not
yet accepted, and cancels running ones, which get the 5 s cancel grace. The stop goes out ahead
of any call notice still buffered for the link, and the link ends after it.

**Bounded cancellation.** A call that is being cancelled ends within 6 s, whether the caller
cancelled or left, the service was stopped, or the authorization was withdrawn. That holds even
when the service stopped reading its call and the relay cannot forward the `cancel`. A call ended
this way is `outcome_unknown`, and its audit record has no byte counts. The Runtime never writes
past a frame toward the caller that was cut short; the caller's stream then just ends, which the
caller's Runtime also reports as `outcome_unknown`. The last frame and shutdown toward a caller
that stopped reading are bounded to 5 s.

**Input stall.** A service must keep reading its call's input. When one caller frame cannot be
handed to the service for 30 s, the call counts as cancelled and ends `outcome_unknown` within the
6 s above. Without this, a caller's `cancel` queued behind input the service never reads could
never be seen.

**Restart.** After a Runtime restart every started service is `unavailable` until its process
attaches again. A service process restart is the same: the old link's end, then a new attach. The
SDK attaches again with backoff from 250 ms doubling to 5 s.

## Operating a hosted service

On the hosting Endpoint, with its daemon running:

```bash
ma2a-example-service manifest 4 > example.json
ma2a service register example --manifest example.json
ma2a service start example
ma2a service expose service SPACE_REF example
ma2a space grant SPACE_REF CALLER_ENDPOINT_ID --service   # replaces every other grant bit
ma2a-example-service serve "$STATE_DIR" example &
ma2a service hosted --json
```

`STATE_DIR` is the daemon's state directory. `ma2a service hosted` shows `ready` once the process
has attached. On the calling Endpoint:

```bash
ma2a service discover --endpoint HOST_ENDPOINT_ID
ma2a service call --endpoint HOST_ENDPOINT_ID example echo --body hello
ma2a service call --endpoint HOST_ENDPOINT_ID example count --body 5
printf 'bytes' | ma2a service call --endpoint HOST_ENDPOINT_ID example pipe --stdin
```

`call` writes the service's output channel to stdout followed by the result, and its error channel
to stderr. Ctrl-C cancels the call, and `--request-id` (32 hex characters) makes a retry replay
instead of calling twice. `--request-id` needs `--version`: a retry must be the identical call,
and choosing the version by discovery could pick another one and would need the target to be
reachable, while a retry of a recorded call is answered locally. A failure prints its status name first, for example `unknown_service:`.

To retire the service: `ma2a service stop example` (the SDK's `serve` then returns), and
`ma2a service unregister example`.

## Status codes

Hosted services add one code to the Services v1 status set:

| Code | Name | Meaning |
| --- | --- | --- |
| 101 | `unknown_service` | not registered, or not available to this caller |

Other codes used:

| Code | Used for |
| --- | --- |
| `unauthorized` | coarse admission; a call whose authorization was withdrawn |
| `version_mismatch` | the `open` envelope; a version the manifest does not list |
| `invalid_input` | malformed bodies; an operation the version does not list; whatever the service refuses as such |
| `unavailable` | a stopped or unready service; a call withdrawn before the service accepted it |
| `concurrency_exceeded` | stream bounds and `max_calls` |
| `conflict` | a RequestId reused with a different call |
| `cancelled` | a call the caller cancelled, as the service reports it |
| `outcome_unknown` | the service connection or the hosting Runtime was lost after delivery |

## Fault points (test-only)

Debug builds only. `MA2A_TEST_SERVICE_FAULT=<point>` makes the daemon kill itself
(`ma2a_services::supervisor::crash()`) at one named point. Release builds compile the check out.

| Point | Window |
| --- | --- |
| `after_call_delivered` | the service received the call, and nothing was relayed yet |
| `before_outcome_relayed` | the service's terminal was read and not yet forwarded |

The caller then settles `outcome_unknown`, and the service sees its call and link end and attaches
again after the restart.

## Known limitations

- **Not a sandbox.** A service process runs as the daemon's OS user and can use everything that
  user can, including the local API (`docs/private-ipc.md`).
- **The Runtime does not supervise the process.** The operator starts and restarts the service
  program; the Runtime only tracks its link.
- **No durable calls.** A call lives as long as its streams. Effects keyed by caller and RequestId
  are the service's own responsibility.
- **Backpressure reaches the caller.** A service that stops reading its input stops the caller's
  input too. The caller's own `cancel` then waits behind the input it already sent, so such a call
  ends when the service reads or exits, on a stop or a withdrawn authorization, or at the latest
  after the 30 s input stall plus the 6 s bound.
- **Payload bounds are the caller's to check.** The Runtime relays a service's `accept` and
  `outcome` verbatim within the frame bounds. A non-SDK service that exceeds the 8 KiB header or
  the 60,000-byte result reaches the caller as a malformed frame, and a replay of such a call may
  answer `outcome_unknown`.
- **Before the accept.** A call waits up to 10 s for its service to accept it. The caller's stream
  is not read meanwhile, so a caller that leaves holds its slot until then.
- **Compatibility.** Phase 4 Runtimes cannot decode a signed member entry that carries `service`
  (bit value `512`), so a Space owner should grant `service` only once every member runs Phase 5.

## Deviations from the plan

The plan is `.omo/plans/ma2a-phase-5.md`. This document describes the code; it differs from the
plan as follows.

- **Operation bounds.** An operation name is 1–32 bytes, not 64, and a version lists 1–16
  operations, not 32, with at most 64 in a manifest, so a manifest always fits 4 KiB.
- **Listing command.** The local command is `service_hosted_list`, answered by `hosted_services`.
- **Accept command field.** `service_accept` names the call as `call_id`.
- **Unary answers.** A unary call may be answered with an `outcome` alone, without an `accept`.
