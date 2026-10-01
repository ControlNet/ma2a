# Phase 4 Agent E2E pitfalls

## Supervisor socket paths cap the state directory length

- Every hosted Process, including every Agent instance, gets a supervisor socket at
  `<state_dir>/run-v1/p/<process-id>…`.
- `ProcessManager::launch` (`crates/ma2a-runtime/src/processes/launch.rs`) refuses
  addresses longer than `MAX_SOCKET_PATH` = 103 bytes, which is the macOS `sun_path` limit.
  The Process then ends as `failed` / `spawn_failed`, with `started_at_ms: null`.
- An Agent whose instance cannot launch restarts again and again, so its `generation`
  climbs, and it never becomes `ready`.
- E2E state directories are `/tmp/ma2a-services-<label>-<role>-<pid>-<counter>`. A long
  test label plus a two-digit counter pushes the path over the limit. The test then fails
  only in some parallel runs, which looks like a race.
- Fix: keep labels short (`f-recorded`, not `fault-after-tool-recorded`).
- To diagnose: `ma2a process inspect <instance process_id>` shows `reason`. The Agent
  view's `instance.process_id` names the instance.

## Hosted Processes outlive the daemon

- Agent instances are hosted Processes, and supervisors outlive the daemon by design.
- E2E teardown must `agent stop` every Agent before shutting the daemon down. Otherwise
  instances leak past the test.

## Restart reconcile and the loopback IPC path

- On restart, pending tool calls re-run before the IPC server has installed its loopback
  paths.
- Remote tools (`crates/ma2a-runtime/src/agents/tools/remote.rs`) wait up to 30 s for the
  loopback, and give up early on shutdown, instead of answering `unavailable` at once.

## Agent refusals

- Coarse admission (`AgentAdmission`) needs the `agent` grant plus at least one exposed
  Agent in the same Space. Failing it gives `unauthorized`.
- Past admission, an Agent the caller may not use is mapped to `unknown_agent` in
  `stream_host/inbound.rs`. That is identical to an undefined Agent.
- Another creator's task is `unknown_task`.
- `ma2a agent --endpoint X submit --from-agent NAME` records attribution only.

## Member-to-member reach needed a control-sync fix (be0a139)

- Every Endpoint's first address record has sequence 0.
- The control pull used to send a record only when `sequence > cursor`, and an absent
  cursor entry defaulted to 0. So a member that joined later never received earlier
  members' first records: B was unreachable from C, and every attempt answered
  `unavailable`, until B refreshed its record about 5 minutes later.
- `ma2a_net::is_unseen` now treats an absent cursor entry as unseen.
- To diagnose:
  - `ma2a space sync status --endpoint X --json` lists the peers this node can sync with
    in the Spaces it shares with X.
  - `state-v1.sqlite3` table `address_state` holds the records each node has.
- Previous E2E tests reached only the owner, whose record arrives in the join bootstrap,
  so they never exposed this.

Audit lessons from the Phase 4 freeze (agent runtime):

- **Every Agent-controlled byte needs a bound below the Store `CHECK`.** A line whose apply hits a `CHECK` would otherwise fail its transaction, never advance the cursor, and stall the link forever. Bound at the decoder, and as a last resort reapply a `ConstraintViolation` as an ignored line.
- **Non-terminal states count toward the durable-event bound;** a terminal state is always recorded.
- **Stop must end the instance in the same transaction that disables the Agent,** and a launch that binds after that must terminate its own Process.
- **Derived start RequestIds must include the definition's creation time,** or a redefined Agent replays its predecessor's start.
- **Open workspace files with `O_NONBLOCK` and check `is_file`:** a FIFO blocks the reader forever.
- **Never remove a `.ma2a-partial` you did not create.**
