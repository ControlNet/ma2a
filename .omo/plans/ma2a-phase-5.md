# ma2a-phase-5 — Service Ecosystem

Baseline: Phase 4 freeze `af7a23b` (Agent Runtime / A2A, frozen infrastructure).

## Goal

Generalize the service architecture from the eight real service families that now exist (Echo,
Files, Transfer, Exec, Shell, Proxy, Process, Agent), and let a third-party program serve a
service through a Runtime:

```text
third-party service process
↔ local MA2A Runtime IPC (same OS user)
↔ authenticated remote MA2A peer
```

The Runtime keeps network identity, remote authentication, Space authorization, resource
admission, and protocol framing. A third-party service never touches the network, never sees a
Space, and only ever receives calls the Runtime has already authorized.

Out of scope: advanced Space governance, cross-Space permission composition, a workflow
scheduler, a global registry, in-process plugins, and schema or code generation.

## What the existing services share (5.1)

Evidence from the eight families:

| Dimension | Shared shape | Differs by family |
| --- | --- | --- |
| identity | a closed `ServiceKind`, one ALPN, one wire code | — |
| version | the `open` envelope `[1, 0]`; one protocol version per family | — |
| operations | one operation per stream, named for the audit | the operation set |
| interaction | Services v1 frames | unary (Files, most Process and Agent reads), structured stream (Agent `watch`, Process `logs`), raw bytes (Transfer, Exec, Shell, Proxy, Process `attach`), long-lived resources (Process, Agent tasks) |
| limits | per-peer and Runtime-wide stream bounds, admitted before any byte | the bounds |
| authorization | coarse admission before reading, then fine access for the decoded request, re-checked while it runs | the access rule |
| cancellation | `cancel` frame or stream loss; withdrawn authorization stops it | how a service reacts |
| replay | mutations use caller-side durable replay | which operations mutate |
| observability | content-free operation audit | — |
| lifecycle, health | none: built-ins exist exactly while the Runtime runs | — |

So the framework is the **pipeline** every stream goes through, driven by a **catalog** of
descriptors; dispatch stays an exhaustive typed match (a trait object would erase the typed
bodies and buy nothing). Lifecycle and health are new, and exist only for hosted services.

## Architecture decisions

### S1 — Model

| Type | Definition |
| --- | --- |
| `ServiceName` | 1–63 bytes of `[a-z0-9-]`, starting with a letter, no trailing `-`. The built-in family names and `service` are reserved. |
| `ServiceVersion` | a protocol major version, `u16` 1–65,535. |
| `OperationName` | 1–64 bytes of `[a-z0-9_.-]`, starting with a letter. |
| `Interaction` | `unary` 0, `stream` 1, `bytes` 2, `resource` 3: what an operation's frames look like. Descriptive; the Runtime enforces the Services v1 frame rules for all of them. |
| `ServiceManifest` | description (≤ 256 bytes); 1–8 versions, strictly ascending, each with 1–32 distinct operations and their interactions; `max_calls` 1–64. |
| `ServiceDescriptor` | what discovery returns: name, `builtin` or `hosted`, description, versions with operations, `available`. Never a Space, a member, or an authorization path. |

### S2 — Catalog and the common pipeline (5.4)

- `ma2a_core::service::catalog` holds one entry per built-in family: its descriptor (versions
  `[1]`, operations with interactions), its coarse admission, and its stream limits.
  `admission(kind)` and `ServiceLimits::of(kind)` read from it, and a test asserts that every
  `operation_name` of every typed body is in its family's descriptor.
- Every inbound stream, built-in or hosted, runs one pipeline: coarse admission (from the
  catalog) → bounded `open` → target and family check → fine authorization → withdrawal watch →
  operation registry → dispatch → report.
- Hosted services are one more arm of the same dispatch: their descriptor comes from the Store,
  not the catalog.

### S3 — Authority

- Member capability bit 9 (value 512) is `service`, signed and default off. As in Phases 3 and 4,
  an older Runtime refuses a member entry that carries it.
- Exposure kind 6 `service` carries one `ServiceName`: `ma2a service expose service SPACE NAME`.
- `ServiceAccess::Discover`: both Endpoints are members of one Space. It is the coarse admission
  of the `service` family.
- `ServiceAccess::Hosted(name)`: in one Space, both are members, the caller holds `service`, and
  this Runtime exposes `name`. A name that is not registered, not exposed to the caller, or not
  granted answers `unknown_service`, identically. Grants and exposures never combine across
  Spaces, and the Space is never disclosed.
- Withdrawal: the existing watch cancels a running call (relayed `cancel`, 5 s grace), and its end
  is `unauthorized`.

### S4 — Transport

- ALPN `ma2a/service/1`, `ServiceKind::Service`, wire code 9; bounds 16 per peer, 64 per Runtime.
- `open` body `ServiceRequest`:

| Operation | Class | Frames after `open` |
| --- | --- | --- |
| `describe {after?}` | read | outcome: a page of at most 16 descriptors, built-ins first, then hosted by name, with `next` |
| `call {service, version, operation, body}` | mutation | whatever the service sends: `accept`, `data`, `end`, `control`, then `outcome`, or `reject` |

- `body` is at most 8,192 opaque bytes. Calls use caller-side durable replay like Exec. The service
  receives the caller Endpoint and RequestId, so it can make its own effects idempotent.

### S5 — Versioning (5.8)

- The `open` envelope stays exactly `[1, 0]`; anything else is `version_mismatch`. The
  `services-v1.md` sentence saying only a different major is rejected is corrected.
- Every built-in family speaks version 1, and its descriptor says so.
- A hosted service declares its versions in its manifest. A `call` names one exact version; a
  version the manifest does not list is `reject(version_mismatch)`, and an operation that version
  does not list is `reject(invalid_input)`. Nothing is negotiated by the Runtime.
- Selection is the caller's, deterministic: the highest version both the caller supports and the
  descriptor lists (`ma2a_service::select_version`). The chosen version is visible to the caller,
  to the service (on every call), and in the per-version call counters of `services_hosted`.

### S6 — Hosting and lifecycle (5.5, 5.6)

Durable (Store schema 10): `hosted_services(service, manifest, max_calls, enabled, created_at_ms,
updated_at_ms)`, and exposure kind 6.

| Local API | Effect |
| --- | --- |
| `service_register {service, manifest}` | creates the registration, stopped; replaces it only while stopped |
| `service_unregister {service}` | only while stopped; its exposures stay, as consent rows, and serve nothing |
| `service_start {service}` | enables it; it becomes `ready` once a link attaches and reports ready |
| `service_stop {service}` | disables it, sends `stop` on the link, cancels running calls, and closes them after 5 s |
| `services_hosted {after?}` | registrations with state, link, and counters |
| `service_attach {service}` | upgrades the connection into the service's **link** |
| `service_accept {service, call}` | upgrades the connection into one **call** |

States: `stopped` (disabled), `unavailable` (started, without a ready link), `ready`. Only
`ready` is advertised as available, and only a `ready` service receives calls; otherwise a call is
`reject(unavailable)`.

**Link.** At most one per service; a second attach is `conflict`, an attach while stopped is
`unavailable`. After `service_attached {manifest}` the connection carries local-API frames with
JSON messages:

- service → Runtime: `{"t":"ready"}`, `{"t":"unavailable"}`;
- Runtime → service: `{"t":"call","call":"<32 hex>"}`, `{"t":"stop"}`.

The link's end (EOF) makes the service `unavailable` at once. There is no heartbeat: the OS
closes the socket of a dead process.

**Call.** A call notice carries only its ID. The service opens a fresh connection with
`service_accept`; the answer `service_call {caller, request_id, version, operation, body}` is the
first time the service learns the call, so if no accept arrives within 10 s the call is withdrawn
and answered `reject(unavailable)` with no possible effect. After the answer, the connection
carries Services v1 frames verbatim through the existing stream-plane relay, which enforces the
direction rules: the service may send `accept`, `data`, `end`, `control`, and one terminal; the
caller's `data`, `end`, `control`, and `cancel` are forwarded. A call whose service connection is
lost ends with `outcome(outcome_unknown)`.

**Saturation.** More than `max_calls` running calls of one service is `reject(concurrency_exceeded)`,
besides the family's stream bounds.

**Restart.** Registrations and `enabled` are durable; links are not. After a Runtime restart every
started service is `unavailable` until its process re-attaches; the SDK reconnects with backoff
(250 ms doubling to 5 s). A service process restart is the same: the old link's EOF, then a new
attach.

### S7 — Discovery (5.3)

`describe` returns, for this caller only:

- every built-in family whose coarse admission the caller passes (Echo through its own policy
  check), with `available = true`;
- every registered hosted service the caller may call (`Hosted(name)`), with `available` true only
  while it is `ready`.

There is no global registry: discovery is always one caller asking one target Endpoint.

### S8 — SDK (5.7)

New crate `ma2a-service`, Rust-first, over the language-neutral link protocol above:

- `ServiceHost`: handlers per `(version, operation)`; `serve(paths, name)` attaches, refuses to run
  when a registered operation has no handler, reports ready, accepts calls concurrently, and
  reconnects after a lost link; it returns after `stop`.
- `Call`: caller, RequestId, version, operation, body; `accept`, `send`, `recv`, `reply`,
  `fail`, `reject`, and cancellation.
- Caller helpers: `describe`, `call`, `select_version`, through the local API stream plane.
- `ma2a-example-service`: a deterministic example service (not a production service). Version 1:
  `echo` and `upper` (unary). Version 2 adds `count N` (stream, honours cancel), `hold`
  (resource-like, ends only on cancel), and `pipe` (bytes: input channel 0 to output channel 1).

### S9 — Status codes

`unknown_service` 101: not registered, or not available to this caller.

## Local API and CLI

- Commands of S6, plus `stream_open` with kind `service`, and exposure kind `service`; grant
  boolean `service`.
- CLI: `ma2a service register NAME --manifest FILE`, `unregister`, `start`, `stop`, `hosted`,
  `discover --endpoint ID`, `call --endpoint ID NAME OPERATION [--version N] [--body TEXT]
  [--stdin]`, `ma2a service expose service SPACE NAME`, `ma2a space grant … --service`.
- Web: exposure kind `service` and a read-only hosted-services card.

## Deterministic fault points (debug builds only)

`MA2A_TEST_SERVICE_FAULT=<point>` SIGKILLs the daemon at:

- `after_call_delivered`: the service received the call and nothing was relayed yet;
- `before_outcome_relayed`: the service's terminal was read and not yet forwarded.

The caller then settles `outcome_unknown`; the service sees its call and link end, and re-attaches
after the restart.

## Acceptance gate (real multi-Endpoint E2E)

On Endpoints A (caller) and B (host) in one Space, with `ma2a-example-service` on B:

- register, start, expose, grant; A discovers B: the granted built-ins and `example` (versions 1
  and 2, available);
- A calls v1 `echo`, v2 `count` (stream) and `pipe` (bytes); version 3 is `version_mismatch`; the
  helper selects 2;
- cancellation: `hold` ends `cancelled` after the caller cancels;
- unavailable and restart: killing the service shows it unavailable and calls are `unavailable`;
  restarting it makes it available; restarting B's Runtime makes the service re-attach;
- denial: no `service` grant, an exposure in another Space than the grant, and an unexposed name
  all answer `unknown_service`; a non-member is refused at admission;
- saturation: `max_calls` 2 with two `hold` calls makes the third `concurrency_exceeded`;
- stop and unregister: the SDK returns cleanly on stop, and the name is no longer discovered;
- the fault points;
- the built-ins keep working through the pipeline, and Phase 1–4 E2E remain green.

## Freeze record (as built)

Phase 5 is frozen on local `master`. The commits after the Phase 4 freeze (`af7a23b`) are:

- `docs(plan)`: this plan;
- `feat(core)`, `feat(store)`, `feat(runtime)`, `feat(service)`, `feat(web)`: Phase 5 itself. That
  is the Service v1 contract and built-in catalog; Store schema 10; the hosting manager,
  discovery, and local API; the `ma2a-service` SDK, `ma2a-example-service`, the `ma2a service`
  CLI, and the acceptance E2E; and the read-only hosted-services card;
- `test(services)`: a pre-existing flaky redaction check that matched a random-ID substring;
- `docs(protocol)`: `docs/protocol/service-v1.md` and the exact `[1, 0]` envelope version;
- `fix(runtime)`: the freeze-audit fixes; and this freeze commit.

**As built, against this plan.**

- `OperationName` is 1–32 bytes, not 64. A version lists 1–16 operations, not 32, with at most 64
  in a manifest, so the encoded manifest stays within 4 KiB.
- The listing command is `service_hosted_list` (result `hosted_services`), not `services_hosted`.
  `service_accept` names its call `call_id`.
- A unary call may be answered with an `outcome` alone, without an `accept`.
- The CLI adds `--request-id` to `call`. Without `--version` it picks the highest version that
  lists the operation.
- Every deviation is also listed in `docs/protocol/service-v1.md`.

**Audit fixes.** The fresh audit found 1 blocker and 6 non-blocking observations.

- Fixed (blocker): a service that stopped reading its call connection blocked the relay while it
  forwarded caller input. After that, no cancel, stop, or withdrawn authorization could end the
  call, and its slot and stream permit stayed held. A call being cancelled now ends lost
  (`outcome_unknown`) one second past the 5 s cancel grace.
- Fixed: a call that outlived an unregister decremented the running count of a re-registered
  service with the same name.
- Fixed: `ma2a service call` followed a target's discovery pages without a limit; it now reads at
  most 8.
- Documented as known limitations:
  - the Runtime relays a service's `accept` and `outcome` verbatim within the frame bounds;
  - the caller's stream is not read while a call waits up to 10 s for its accept;
  - a caller's own `cancel` waits behind input it already sent to a service that stopped reading.
- Not changed: granting `service` in a Space that still has Phase 4 members, which is already
  documented. There is still no deterministic unit test of the deliver-versus-withdraw race; the
  E2E and IPC tests cover the hand-over.

**Test-only items.**

- `MA2A_TEST_SERVICE_FAULT`, debug builds only.
- `ma2a-example-service`: a deterministic example, not a production service.
- The E2E lab's service registrations.
- The scripted service end in `crates/ma2a-services/tests/client_service.rs`.
- The paused-clock call-relay tests.
- The hosted-services data in `web/src/test/services-fixture.ts`.
- The `repository_record()` helper in `crates/ma2a-runtime/tests/support/fixtures.rs`.

**Verification at freeze**, on Linux x86_64:

| Check | Result |
| --- | --- |
| `cargo run --locked -p xtask -- check` (fmt, clippy `-D warnings` with all features, LOC, deny, machete, nextest, web) | clean, 896/896 tests passed |
| `cargo test -p ma2a-app --test ecosystem_e2e`, three runs | 3/3 each |
| `ma2a-core` and `ma2a-services` (`--features blake3/pure`) clippy for `aarch64-apple-darwin` and `aarch64-pc-windows-msvc` | clean |
| web: `biome check`, `tsc --noEmit`, `bun test` | clean, 149 tests passed |

`ma2a-service` depends on `ma2a-runtime` (`ring`), so it is cross-linted only by native CI.

Two flakes appeared under concurrent load and are unrelated to Phase 5:

- once, `process_e2e` `a_restarted_or_crashed_daemon_readopts_its_running_processes`. It passed 3/3
  alone and in the next full run;
- earlier, `ma2a-store` `two_concurrent_redemptions_have_exactly_one_winner` ("database is
  locked").
