# Decisions — ma2a-phase-0-1

Architectural choices and rationales discovered during work on this plan.

_Auto-scaffolded by /start-work. Append new entries below - never overwrite._

---

## 2026-08-29 - Todo 6 Runtime Endpoint

- Use one stable protected-key reference and compare its derived public identity with SQLite before
  transport bind; any disagreement is a fail-closed startup error.
- Use Iroh Minimal plus explicit private components rather than N0 presets or public discovery.
- Keep synchronous storage in one bounded blocking owner and all Runtime mutation in one bounded
  async actor so shutdown can join exactly two owned tasks.
- Treat persistence as the commit point for actor-visible membership state; failed observations do
  not mutate the authoritative snapshot or emit a change event.
- Report the final persisted observation revision on unclean cancellation; clean shutdown continues
  to report the later clean-metadata revision.

## 2026-08-29 - Todo 7 Windows IPC correction

- Keep Windows SID and impersonation FFI inside `ma2a-runtime` as one private audited module rather
  than expanding the accepted workspace with a platform helper package.
- Authorize each named-pipe connection by exact equality between the daemon process-token SID and
  the impersonated client thread-token SID, with explicit and Drop-based reversion paths.
- Preserve `unsafe_code = "deny"` at workspace and crate scope so only the audited module can use a
  reasoned local allow; enforce this confinement and exact workspace membership in `xtask` tests.

## 2026-08-30 - Todo 14 control synchronization corrections

- Keep local address and relay publication actor-owned and route persistence through typed store mailbox commands, so scheduling occurs only after committed advancement.
- Preserve explicit peer intent in the pending-round scope and retain peer identity on waiters, allowing completion to be tied to the exact authenticated synchronization outcome.
- Keep raw address advancement private to the store and accept only signed, authorized `ValidatedAddressRecord` values at the persistence boundary.

## 2026-08-31 - Todo 21 Phase One E2E

- Keep Todo 21 implementation test-only: enable pinned Iroh test utilities for `ma2a-app`, compose accepted fixtures, and add no production transport or Runtime APIs.
- Emit public Endpoint IDs, record sequences, candidate relay URLs, observed home/path labels, reachability state, and high-water values as JSON; never emit protected key bytes or fixture credentials.
- Run the exact focused target three times in CI and upload logs plus a concise repetition summary even when a run fails.

## 2026-09-01 - F2 durable local mutation replay

- Persist local mutation replay in schema v4 with pending and completed states. Reserve before dispatch so restart cannot silently execute an ambiguous request twice; identical pending retries return `unavailable` and changed fingerprints return `conflict`.
- Retain exact encoded successful responses rather than reconstructing typed results, preserving byte-for-byte replay and shutdown semantics across process restart.
- Bound completed decisions to 1,024 rows, 8 MiB aggregate response bytes, and 65,536 bytes per response. Evict deterministically by persistent `(sequence, request_id)` FIFO order while never evicting pending reservations.
