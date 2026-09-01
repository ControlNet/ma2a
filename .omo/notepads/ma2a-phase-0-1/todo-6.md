# Todo 6 Runtime Endpoint

- Implemented in isolated worktree `/tmp/opencode/ma2a-todo-6` from exact base
  `007625727bbd960f16745b3cb97953c43641897d` on branch `todo-6-runtime-endpoint`.
- The Runtime owns one bounded actor mailbox, one bounded event broadcast, one cancellation tree,
  one Iroh router, and one dedicated blocking SQLite/key-store owner.
- The protected key slot is the stable opaque reference `endpoint-identity-v1`. Missing material is
  generated only when no Endpoint record exists; corrupt, missing-after-record, mispermissioned, or
  public/private-mismatched material fails closed without replacement.
- Iroh uses `presets::Minimal`, disabled relays, cleared public address lookup, a private memory
  lookup, and enrollment-only ALPN registration. Normal MA2A ALPNs fail during negotiation.
- Membership observations replace only the in-memory membership set and persisted observation;
  they do not rebuild or rotate the Endpoint.
- Graceful shutdown closes the Iroh router, persists not-ready and clean-shutdown metadata, stops
  the blocking owner, and proves both owned tasks joined.
- Focused verification passed 5 Runtime identity tests and 1 real Iroh zero-Space test. The locked
  aggregate gate passed 69 Rust tests, dependency/license/source audits, 41 Web tests, and builds.
- Rust LSP diagnostics could not target the isolated `/tmp` worktree because the tool rejects paths
  outside the request cwd; compiler, strict Clippy, rustfmt, nextest, and live Iroh tests passed.
