# Todo 18 Snapshot And SSE

- Runtime snapshots must be built through the actor mailbox so transient Endpoint/control/Echo state and durable SQLite state share one authoritative API path.
- Durable snapshot fields are read in one SQLite transaction. This prevents revision and payload fields from describing different commits.
- Session validation for long-lived SSE must not touch sliding deadlines. Heartbeat validity checks otherwise become durable mutations and create artificial revision events.
- Web uses `LocalApiClient::snapshot_fetch`; it does not open a second Runtime business-logic path.
- Best-effort SSE accepts only the current baseline. It emits consecutive revision invalidations and closes with `resync-required` for stale baselines, gaps, lag, Runtime failure/restart, or invalid sessions.
- The current schema requires a non-empty Space name but persistence has no local label yet. The canonical lowercase Space ID is the truthful temporary display name.
- Reuse the Runtime API codec's canonical lowercase hexadecimal encoder for snapshot identifiers instead of maintaining a second byte-to-hex implementation.
- Group Runtime-backed Web dependencies into a named construction type and keep each SSE connection's state in an owned producer object; this preserves permit lifetime and keeps async entry points below the workspace argument limit.
- Targeted Biome checks remain usable even when the repository-wide Biome configuration has an unrelated compatibility problem.
