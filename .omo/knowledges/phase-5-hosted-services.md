# Phase 5 hosted services: design facts and pitfalls

What was learned building hosted services (`crates/ma2a-runtime/src/hosting*`, the
`ma2a-service` SDK, `ma2a service` CLI, `tests/ecosystem_e2e`). The protocol is in
`docs/protocol/service-v1.md`.

## Call hand-over without an ambiguous effect

- The link only carries a call ID. The service learns the call from the `service_accept` answer.
- The IPC accept handler removes the pending call under the state lock, pre-encodes the
  `service_call` answer, and passes the stream plus answer bytes to the call task through a
  oneshot. The **call task** writes the answer, so withdrawal and delivery are decided by one lock
  holder: a call is never both delivered and withdrawn.
- On timeout (10 s) or stop, the task tries `withdraw(call)`. If that fails, the accept handler
  already took it: await the oneshot instead of refusing.

## Cancel safety

- `ma2a_runtime::ipc` `read_reply` is **not** cancel-safe: dropping it in `select!` can split a
  frame. The SDK reads link messages in a spawned task that feeds an mpsc channel.
- `ma2a_services::FrameReader::next` **is** cancel-safe (it keeps partial frames).
- Stop or withdrawal must reach a running call as `cancel`. The runtime forwards caller frames
  through a `tokio::io::duplex` pump that is the relay's client reader. The pump injects `Cancel`
  and returns, and the relay's own 5 s cancel grace then applies.
- Polling a completed `async fn` future again panics. When reusing `client::process::inbound` for a
  cancel future, let only one place await it.

## Replay comes from the caller side

Hosted calls are mutations (`ServiceRequest::is_mutation`), so the generic caller-side replay in
`ipc/server/stream.rs` covers them:

- the same RequestId and the same call replays the recorded terminal without reaching the service;
- the same RequestId with a different body is `conflict`.

The E2E checks that the per-version call counter does not move on a replay.

## Error names the CLI shows

| Situation | Error |
| --- | --- |
| an Endpoint sharing no Space | `unavailable`: it cannot even locate the target, because address lookup is Space-scoped |
| a removed member, while its address is still cached | `unauthorized` |
| a member without the grant, an unexposed name, or a grant in another Space | `unknown_service` |
| a lost program or hosting daemon after delivery | `outcome_unknown` |

Without `--version`, `ma2a service call` discovers the service first. If it is not listed, the CLI
answers `unknown_service: the target serves this Endpoint no service NAME` locally.

## A service that stops reading

The relay forwards caller input to the service. If the service never reads, `forward` blocks
forever: the `cancel` behind the input is never read, and the relay's cancel grace never starts.
`hosting/call/relay.rs` therefore runs its own timer, `STUCK_GRACE` (`CANCEL_GRACE` plus 1 s),
started by a caller cancel or departure that the pump sees, by `stop`, or by `service_stop`. When it
expires, the relay future is dropped and the call ends lost. The paused-clock tests
(`#[tokio::test(start_paused = true)]` with a `timeout` bound) prove it, and hang without the fix.

## E2E pitfalls

- Building `ma2a-example-service` from inside a test with `cargo build -p ma2a-service`, every
  run, rebuilds shared dependencies (ring, rustls, iroh) with a different feature set, which costs
  about 100 s per run. Build it only when absent, as `example_agent()` does, and refresh it with
  `cargo build --workspace --bins` after changing the SDK or runtime.
- Send Ctrl-C to a background CLI with `kill -INT PID`. The CLI must install its handler before
  the signal arrives: wait until `service hosted` shows the call running.
- New workspace crates must be added to `EXPECTED_MEMBERS` in `xtask/src/workspace_tests.rs`, or
  `cargo xtask check` fails.
- LOC policy (250 pure lines) bit `web/src/api/services.ts`. The Agent schemas now live in
  `services-agents.ts`.

## Pre-existing load flakes

`process_e2e` `a_restarted_or_crashed_daemon_readopts_its_running_processes` failed once while
another cargo build loaded the machine. Its `process inspect` reported `outcome_unknown`. It passes
3/3 alone, taking 35–60 s.

The Store test `two_concurrent_redemptions_have_exactly_one_winner` once failed with "database
is locked" under full-workspace load. It passes 8/8 when run alone, and is unrelated to Phase 5.
