# Additional review findings — 2026-11-13

Reviewed revision: `ce387ad`. Complements `2026-11-13-deep-project-review.md` (R1–R6) and
`2026-11-13-deep-project-review-round-2.md` (R7–R10), which are not repeated here. None of the
items below has a reproducing test. N1, H1–H4, H6, H8 and H9 were traced in source by the main
reviewer; H5, H7 and the Minor list come from a sub-reviewer, with H5 and H7 spot-checked. The
line-level pass covered only Phase 5 (`d856f11..ce387ad`). No production code was changed.

## N1 — P1: abandoned replay reservations can disable every local mutation

- `ipc/server.rs` `serve` ends with `tasks.abort_all()`. A mutation stream still running at that
  moment (exec, shell, proxy, process attach with stdin, files write, hosted service call, Agent
  tool call) is dropped before `Reservation::complete` or `release`
  (`ipc/server/stream/replay.rs`). A crash leaves the same state.
- The row stays `Pending` in `local_mutation_replay` forever: nothing reconciles pending rows at
  startup, and `enforce_retention` (`ma2a-store/src/mutation_replay.rs`) evicts only completed rows.
- When pending rows exceed `LOCAL_MUTATION_REPLAY_MAX_ENTRIES` (1,024), every
  `reserve_mutation_replay` fails with "capacity is exhausted by pending requests". Local API
  mutations (`ipc/server/dispatch.rs`) share the table, so every local mutation fails closed, with no
  recovery command. Proxy alone allows 256 concurrent streams, so a few restarts with open proxy
  connections can reach the bound.
- Checked: `Reservation` has no `Drop`, and `abort_mutation_replay` / `record_mutation_replay`
  have no startup caller.
- Fix direction: reconcile pending rows at authoritative daemon startup while holding the daemon
  lifetime lock, not in every `Repository::open` (Web auth opens the Store while the daemon runs).
  Keep the permanent fences so old RequestIds never become executable again, and keep local JSON
  responses distinct from stream terminal frames, since both share the table. Settle reservations
  on graceful shutdown before aborting. Reproduced as R11 in
  `2026-11-13-review-complete-findings.md`, which supersedes this note's numbering.

Related design note: `local_mutation_fences` grows by one row per admitted mutation RequestId by
design (`docs/protocol/local-api-v1.md`). Since Phase 2 every remote mutation and Agent tool call adds
a row, so the growth rate is far above the Phase 1 assumption.

## Phase 5 (hosted services)

| # | Sev | Location | Issue |
| --- | --- | --- | --- |
| H1 | P2 | `ma2a-app/src/commands/remote/service.rs` `call` | `called?` returns before the `--stdin` `process::exit(0)`; the pending blocking stdin read keeps the tokio runtime from shutting down, so any failed or cancelled `service call --stdin` hangs until EOF. |
| H2 | P2 | same, `newest()` before `open_as` | Without `--version`, a `--request-id` retry first runs discovery: an offline target gives `unavailable` instead of the recorded outcome, and a re-registered higher version changes the fingerprint and gives `conflict`. |
| H3 | P2 | `hosting.rs` `observe`, `hosting/call.rs` | Per-version `calls` counters survive re-registration; only unregister clears them. More than 8 called versions makes the web `HostedServicesSchema` (`.max(8)`) reject the whole Services payload. |
| H4 | P2 | `ipc/client/stream.rs` `upgrade`, `ipc/server/dispatch.rs` | Every upgrade (each SDK `service_accept`) first sends `handshake`, which takes the global mutation `replay` lock and builds a full snapshot; a slow local mutation can push accepts past the 10 s `ACCEPT_DEADLINE`. |
| H5 | P3 | `hosting/operator.rs` `stop`, `ma2a-service/src/host.rs` `serve` | `Notice::Stop` is sent with `try_send`; on a full channel it is lost, the SDK sees a lost link and re-attaches forever instead of returning. |
| H6 | P2 | `hosting/call/relay.rs` `relay_call`, `hosting/call.rs` `lost` | When `STUCK_GRACE` fires, the `relaying` future may be dropped mid-frame toward the caller; `lost()` then writes an `outcome` frame after a partial frame, and awaits that write without a bound, so a caller that stopped reading hangs the call task. |
| H7 | P2 design | `ma2a-core/src/service/catalog.rs` | The Service family is admitted on Space membership (`Discover`) while sharing the `(16, 64)` stream slots; an ungranted member can hold all slots for 15 s each by never sending `open`. |
| H8 | P2 design | `ma2a-store/src/hosted_services.rs` `unregister_service` | Exposures outlive unregister (documented); a different program later registered under the same name is immediately exposed to those Spaces without warning. |
| H9 | P3 | `ipc/server/execute/hosted.rs` | Register/start/stop report a revision read after the operation instead of the commit's own receipt, so the response and its replay record can carry a later revision. |

Minor: describe re-reads Spaces and exposures per family (`hosting/describe.rs`); register loads up
to 64 manifests to count them; the example service accepts `manifest 0` or `100`; a dead
`ServiceAccess::Discover` match arm (`ma2a-core/src/service/access.rs`); stale grant-order doc
comments (`api/snapshot/member.rs`, `ma2a-store/src/revision.rs`); the `before_outcome_relayed`
fault hook also fires on `reject`.

## Documentation drift

`README.md`, `docs/architecture.md`, `docs/limitations.md`, `docs/storage.md` (still "schema version
1"), `docs/cli.md` (none of the Phase 2–5 commands), and `docs/security.md` (no threat model for
remote exec, shell, proxy, or the Agent tool bridge) still describe Phase 1.

## SDK dependency weight

`ma2a-service` depends on `ma2a-runtime` only for `LocalApiClient` and `IpcPaths`, so every
third-party service links iroh, rusqlite, and ring, and the SDK cannot be cross-linted without C
toolchains. Extracting the local IPC client into a small crate would fix both.
