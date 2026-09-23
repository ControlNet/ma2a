# Runtime background reconciliation: failure matrix

Source of truth: relay candidates come from signed/configured Store state; the
latest `IrohRelayObservation` is a complete current Iroh snapshot. The Endpoint
relay map, persisted relay observations, `RuntimeStatus`, and signed local
address/relay records are projections. `RuntimeStatus` currently contains only
one relay/observation projection; incomplete effects therefore need explicit
pending state, rather than interpreting equality with that projection as success.

| Operation | Desired source | Current early mutation | Later effects and failure gap | Retry source |
| --- | --- | --- | --- | --- |
| Candidate refresh | Store relay map | `state.relay.replace_candidates` | Iroh map, observation Store write, forced address publication. Any error after mutation makes the next equality check skip the rest. | Periodic Store reload; stage Actor state until all effects complete. |
| Iroh observation | Complete latest stream snapshot | direct reachability, relay observation, endpoint address/data | Observation Store write, `store.observe`, forced address publication. A later failure is lost if Iroh sends no more events. | Retained latest snapshot; newer snapshot supersedes older. |
| Local control publication | Store configuration, current Iroh address | Store commits and Actor revision inside nested calls | Lookup refresh, candidate refresh, control scheduling can fail after signed state advances. | Periodic maintenance plus pending completion stages. |
| Relay advertisement | Store configuration and active Spaces | Sequence reserved before per-Space writes | Per-Space persistence and later candidate refresh/scheduling can fail. | Store-retained signed batch plus Actor completion marker. |
| Private relay access | Store authorizations | None | Load can fail before server replacement. | Explicit mutation paths; no background equality gate here. |

`refresh_control_lookup` loads Store state then atomically replaces its lookup
projection. Iroh relay reconfiguration compares URL sets and is repeat-safe.
Originally, `replace_relay_observations` always incremented revision. Address
publication with `force_advance` was not repeat-safe.

## Implemented model

- Store candidates are desired. `refresh_relay_candidates` clones the applied
  Actor relay state, applies the desired map to Iroh, persists filtered staged
  observations, and runs idempotent address publication before committing the
  Actor relay state. Failure leaves Store desired state available for the next
  periodic attempt. Iroh map application compares URL sets, so replay is safe.
- Iroh observations are complete snapshots. The Actor retains the latest one in
  `pending_observation`, superseding an older pending snapshot, and stages
  `RuntimeStatus` before observation and publication effects. It clears pending
  only after all required effects finish. The periodic tick retries without a
  second stream event. Direct reachability and endpoint data are applied with
  the staged status.
- Relay observation writes and background endpoint metadata retries compare
  effective content before incrementing revision. Explicit membership
  observations still advance revision even when the member count is unchanged,
  preserving their event stream semantics. Address publication uses its natural
  content/renewal check, not `force_advance`, so partial per-Space success is
  skipped on retry. `address_lookup_pending` survives a Store or lookup failure
  until lookup refresh and one control trigger finish.
- The Store backend retains a signed relay advertisement batch after sequence
  reservation. Failed per-Space writes or activity reconciliation retry those
  same bytes/sequence only while the retained batch is outside the shared
  five-minute renewal window. At 299,999 ms after issue, a ten-minute batch
  may still replay; at 300,000 ms, it is replaced. The completed Store
  publication result reports the actual signed issue/expiry window, including
  when the retry request arrived later. The Actor records this Store result in
  `published_relay`; retry request time never extends signed validity. Entering
  the renewal window deliberately replaces a partial batch with a fresh
  sequence in the same recovery turn. This prevents a successful recovery from
  leaving an almost-expired advertisement until the next maintenance tick.
  A Space missing the old sequence may advance directly to the replacement
  sequence. If replacement persistence partially fails, its exact signed batch
  becomes the new pending batch. Expiry also forces replacement, while a clock
  rollback before the retained issue time fails closed. The Actor also remembers
  configuration, authorizations, and `relay_followup_pending`; a retry of
  candidate refresh/scheduling does not publish another signed sequence.
  Actor maintenance and Store replay use the same policy: age strictly under
  five minutes and more than five minutes remaining before expiry.
- `RuntimeStatus` relay candidates and Iroh observation fields represent the
  applied projection. A pending attempt keeps the previous Actor revision even
  if Store has committed an intermediate effect. On successful projection
  commit, it adopts the new Store revision or advances one if all durable
  effects were no-ops; different applied relay/endpoint projections cannot
  share a revision.
  Signed address/relay records and persisted observations are durable
  projections, not independent desired sources.
- Background maintenance retries only Store `Interrupted`/`WouldBlock`/`TimedOut`
  I/O, SQLite busy/locked errors, and an address publisher awaiting its first
  live Iroh observation. Publisher-wrapped Store errors are unwrapped for this
  policy. Closed Iroh Endpoint reconfiguration, poisoned/closed observations, schema/integrity,
  identity, signed-state, clock, task, and channel failures propagate and stop
  the Actor. Every failed attempt returns to the event loop; the periodic tick
  is the bounded retry schedule.

The Actor's signed relay renewal marker is in memory; a fresh Runtime boot
deliberately republishes the local relay advertisement once, preserving the
existing startup refresh behavior. A process crash during a partially written
relay batch still has Store high-water rows but loses the in-memory signed batch;
the next boot issues a new sequence and converges rather than replaying the
previous batch.
