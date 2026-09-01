# Todo 4 Local API v1

- Keep local API semantics transport-neutral so IPC and loopback HTTP adapters share one typed contract.
- Parse the version field before operation fields or callbacks; incompatible clients must not trigger replay, state, or mutation work.
- Canonical typed-command fingerprints make RequestId replay deterministic without retaining untrusted input shapes.
- Keep public closed enums behind invariant-preserving wrappers when workspace exhaustive-enum lints conflict with a frozen external discriminant inventory.
- Split generated TypeScript payload models from exhaustive consumers when formatting would push one authored source file beyond the 250-pure-LOC ceiling.
- Best-effort SSE must treat disconnects, duplicates, reordering, and revision gaps as resnapshot conditions rather than pretending to provide durable replay.
- `serde_json::Value` cannot enforce canonical object members because duplicate keys are already collapsed; retain duplicate metadata in a custom root `MapAccess` visitor before version and command validation.
- Version-first ordering is compatible with duplicate rejection when a single incompatible numeric version wins before duplicate non-version fields, while duplicate `version` fields remain ambiguous `invalid_input`.
- Link TypeScript field inventories to `keyof` models with exact-key type checks, then compare those inventories to the structural machine schema at runtime; this catches both missing and added model fields without comparing two hashes from the same unchecked source.
- Closed protocol error wire names belong on the private-enum-owning type so an exhaustive `match` gives compiler-enforced serialization coverage.
