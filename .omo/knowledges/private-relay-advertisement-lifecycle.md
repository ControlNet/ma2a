# Private Relay Advertisement Lifecycle

- Durable withdrawal uses an `active` flag on `relay_advertisement_state`; never delete accepted
  rows because their sequence and hash remain the rollback/fork high-water after restart.
- Schema migration 5 marks existing version 4 advertisement rows active, preserving prior behavior.
- Higher-sequence accepted advertisements reactivate their row. Idempotent replay does not reactivate
  a withdrawn row, so only a genuinely newer signed artifact can restore visibility.
- `ControlSpaceState::relay_advertisements` retains all rows for cursor construction, while
  `active_relay_advertisements` is the consumer boundary used by control pages and relay selection.
- Publishing reconciles the provider's complete active Space set in the same backend operation.
  Removed served Spaces are withdrawn even when no replacement advertisement is generated for them.
- Actor startup, periodic local-control maintenance, and relay configuration mutation all call the
  same relay publication refresh path. Disabled configuration reconciles the active set to empty.
- Lifecycle coverage must prove configure publication, served-Space removal, disable withdrawal,
  restart refresh, periodic sequence refresh, inactive consumer filtering, and preserved high-water.
- Verification on 2026-09-02: all `ma2a-store` and `ma2a-runtime` tests passed; strict clippy passed
  for `ma2a-store`, `ma2a-net`, and `ma2a-runtime`.
