# Todo 13: Private relay separation

- Iroh 1.1.0 `RelayMode::Custom(RelayMap)` is the explicit public transport fallback seam; the
  default remains `RelayMode::Disabled` with the existing MA2A `SpaceAddressLookup`.
- `iroh-relay` server `AccessControl::on_connect` receives the handshake-authenticated Endpoint ID,
  so admission can use an atomically replaced union of current served-Space members.
- Native embedded TLS maps to `CertConfig::Manual`; external termination requires a loopback-only
  plaintext backend behind an operator-managed compatible HTTPS/WebSocket proxy.
- Native key loading is path-only, bounded, current-certificate checked, key-correspondence checked,
  zeroized where supported, and Unix current-user mode `0600` fail-closed.
- Private relay advertisements are canonical, provider-Endpoint signed, exact-Space scoped, bounded
  to ten minutes, capability checked, and persisted with rollback/fork high-water protection.
- Public fallback is transport-only and has no code path into Space membership, advertisements,
  target EndpointData, or `SpaceAddressLookup`.
- Effective provider Spaces are the intersection of configured served Spaces and current
  `PRIVATE_RELAY_PROVIDER` authorization; configured-but-incapable Spaces publish nothing.
- Relay connections must be indexed by both Endpoint ID and Iroh Connection ID so disconnect
  callbacks prune exactly one connection and final eligibility loss can evict every live client.
- Provider advertisement sequences belong to Store state and must be transactionally reserved
  before signing so restart and concurrent publication cannot reuse a sequence.
- A Store-owned relay configuration keeps persistence types independent from Iroh runtime types;
  the Net boundary parses URLs, listener addresses, and transport mode into runtime values.
- Real HTTPS path tests are required for both native TLS and external termination. Merely starting
  listeners does not prove that an Iroh client can complete the HTTPS/WebSocket relay path.
- Repository pure-LOC policy is 250; relay settings and publication coverage are separate modules
  to keep persistence, validation, and publication responsibilities reviewable.

## Independent review: 2026-08-30

- Verdict: `REJECT` at exact tip `a647729d7b7f848f0cf91d8f2685991c62a89b69` against base
  `39064083b10ae89b90315041c323dbe9d5e1ac6f`.
- Unix private-key validation masks `metadata.permissions().mode()` with `0o777` before comparison,
  so a key with mode `01600` is accepted even though the requirement is exact mode `0600`.
- Windows private-key ACL validation invokes `Get-Acl` on the pathname after opening the file. It
  therefore validates a second path lookup rather than the already-opened handle, leaving a TOCTOU
  gap against the opened-handle requirement.
- `Repository::advance_relay_advertisement` is public and accepts a publicly constructible
  `RelayAdvertisementAdvance`, then persists its supplied hash and signed bytes without signature,
  capability, Space-scope, expiry, or hash validation. A disposable driver confirmed arbitrary
  advertisement bytes can be committed through this API.
- A disposable concurrency driver confirmed 16 simultaneous provider sequence reservations produced
  unique values `1..=16`; the probe and its build artifacts were removed after the review.
- Focused relay tests passed 20/20, repeated real-client TLS tests passed 2/2, Store transaction tests
  passed 12/12, and aggregate core/store/net tests passed 146/146. Formatting, strict Clippy, LOC,
  aggregate `xtask check`, and `git diff --check` also passed.
- Secret Guard found no staged files, no secrets in 281 tracked files, and complete `.gitignore`
  coverage for its common sensitive-file patterns. No Todo 13 relay process or listener remained.
- LSP diagnostics could not run because the review worktree paths were outside the tool request CWD;
  compiler, Clippy, and test gates supplied the Rust diagnostic evidence instead.

## Remediation: 2026-08-30

- Unix private-key validation now masks `0o7777`, so set-ID and sticky bits are included and only
  exact mode `0600` is accepted; the new `01600` regression fails on the rejected tip and passes now.
- Windows opens the key once with `FILE_FLAG_OPEN_REPARSE_POINT` and validates owner, protected DACL,
  exactly two non-inherited full-control allow ACEs, and current-user/SYSTEM SIDs from that same handle.
- PowerShell and all path-based ACL re-resolution were removed. Every Win32 API failure is fail-closed,
  and raw-pointer reads are bounded by the descriptor, ACL, ACE, and SID sizes returned by Windows.
- Raw `RelayAdvertisementAdvance`, `SequenceOutcome`, and `advance_relay_advertisement` are no longer
  public or present. Only `ValidatedRelayAdvertisement::parse` can produce persistence input after
  canonical bounds, HTTPS, exact Space, provider capability, clock, signer identity, and signature checks.
- A passing `compile_fail` doctest proves external code cannot import `RelayAdvertisementAdvance`.
- Full workspace tests, strict all-target/all-feature Clippy, `cargo run --locked -p xtask -- check`,
  formatting, diff checks, LOC checks, and 44 frontend tests passed. Real native and externally
  terminated HTTPS relay paths passed three consecutive runs (2/2 each run).
- The full Windows target check reached transitive native builds but cannot finish on this Linux host:
  `blake3` required no-NEON mode, then `ring` required an unavailable Windows `clang` toolchain.
  An isolated pinned `windows-sys = 0.61.2` harness compiled and passed strict Clippy for the exact
  Windows FFI modules on `aarch64-pc-windows-msvc` with Rust 1.91.0.
- LSP diagnostics remained unavailable because the isolated worktree is outside the request CWD;
  native compiler, strict Clippy, Windows-target compiler, tests, and `xtask` supplied diagnostics.

## Source rebase: 2026-08-30T10:57:10+10:00

- Verified immutable preconditions before rewriting only `todo-13-private-relay`: `master` was exactly
  `f84e0ec55bc30984be186156a8d10e475082eff4`, the approved source tip was exactly
  `056e123c1786af0928166fc13cbadc6041f57c90`, and the old merge base was
  `39064083b10ae89b90315041c323dbe9d5e1ac6f`. The source range contained 15 ordered commits, zero
  merges, no upstream, and no committed `.omo/**`, `target/**`, or `web/dist/**` path.
- Rebased with exactly `GIT_MASTER=1 git rebase --onto master
  39064083b10ae89b90315041c323dbe9d5e1ac6f`. The sole conflict was
  `crates/ma2a-net/src/lib.rs` at old commit `c7a458e`; the resolution retained Todo 10
  `ZERO_SPACE_ALPNS` while adding all Todo 13 private-relay, admission, advertisement, configuration,
  and TLS exports. `master` remained unchanged.
- `git range-diff` maps all 15 old commits to 15 rebased commits in their original order. The new tip
  is `6e68a39c1bf2101179d4653c212d9b0f1ba2b904`; the only semantic range-diff marker is the intentional
  Todo 10 export retention in rebased commit `b2dbc205cf1990eca49326f720dedb211d48188c`.
- Focused verification passed authorization 17/17, zero-Space isolation 2/2, relay behavior 21/21,
  Store transactions 9/9, and the Store compile-fail doctest 1/1. Native and externally terminated
  real HTTPS relay paths passed three separate 2/2 runs.
- Core/Store/Net aggregate verification passed 164/164 tests. Formatting, strict all-target/all-feature
  workspace Clippy, LOC, and locked `cargo run --locked -p xtask -- check` passed; the final aggregate
  ran 243/243 Rust tests and 44/44 Web tests with all dependency and source-policy gates successful.
- Secret Guard found no staged files, no secrets in 292 tracked files, and complete common-sensitive
  `.gitignore` coverage. Diff, merge-count, excluded-path, conflict-marker, raw-persistence API, exact
  process, and temporary relay/TLS/zero-Space residue probes passed.
- LSP diagnostics for the manually resolved Rust file returned `LSP file path must be inside request
  cwd`. Miri was attempted but `cargo-miri` is not installed for the available nightly toolchain;
  compiler, strict Clippy, tests, the audited Windows-target harness evidence, and aggregate gates remain
  the available proof. Final source status has no tracked or staged changes and retains only protected
  untracked `.omo/**` evidence.
