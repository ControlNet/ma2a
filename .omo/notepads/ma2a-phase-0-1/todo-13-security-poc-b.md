# Todo 13 rebased security PoC B

## 2026-08-30 - exact tip `6e68a39c1bf2101179d4653c212d9b0f1ba2b904`

### Verdict

**VERDICT: REJECT**

The rebase equivalence and Linux-observable relay security behavior survived independent
falsification. The exact Windows FFI sources also cross-compile for AArch64 MSVC and the
same-handle design is statically coherent. Approval is nevertheless rejected because the recorded
claim that the isolated Windows FFI harness passed strict Clippy is false at the exact tip: Clippy
rejects `relay_tls_windows.rs` under two denied unsafe-boundary lints already declared by the
repository. This is a platform verification/policy failure, not a reproduced exploit.

### Rebase reconstruction

- Confirmed `HEAD` is exactly `6e68a39c1bf2101179d4653c212d9b0f1ba2b904`; base object
  `f84e0ec55bc30984be186156a8d10e475082eff4`, old approved tip
  `056e123c1786af0928166fc13cbadc6041f57c90`, and old base
  `39064083b10ae89b90315041c323dbe9d5e1ac6f` all exist as commits.
- Both ranges contain exactly 15 commits. `GIT_MASTER=1 git range-diff --creation-factor=80`
  maps all 15 one-for-one and in order. Entries 1-8 and 10-15 are `=`; only entry 9 is `!`.
- The entry-9 delta is confined to `crates/ma2a-net/src/lib.rs`: rebased commit `b2dbc205`
  retains the new base export `ZERO_SPACE_ALPNS` while applying the old `c7a458e` relay module and
  public exports. Comparing old base/new base, old commit/rebased commit, and both final files found
  no lost relay export and no lost Todo 10 export.
- `GIT_MASTER=1 git diff --check f84e0ec..6e68a39` passed. Final source status remained at the
  requested tip with no tracked or staged changes; only the pre-existing protected untracked
  `.omo/` directory was present.

### Safe live probes

- Zero-Space closure: `cargo test --locked -p ma2a-app --test zero_space_isolation` passed 2/2.
  Real Iroh negotiation rejected all five normal ALPNs plus an unknown future ALPN before body
  parsing, for both malformed and oversized payloads, while enrollment remained reachable.
- Relay admission and eviction: `cargo test --locked -p ma2a-net --test relay_admission` passed
  6/6. This included real member/nonmember admission and a live client observing closure after final
  eligibility removal. The endpoint-plus-connection-ID pruning cases also passed.
- Native/external HTTPS: `relay_tls_paths` passed 2/2 in the focused run and then 2/2 for five
  additional consecutive runs. Each run used a real Iroh client; external mode traversed a real
  local rustls terminator and plaintext loopback backend.
- TLS file rejection: `relay_tls` passed 5/5, including symlinked material, oversize bounds,
  mismatch/insecure permissions, and Unix special permission bits.
- Disposable race probe outside the product tree performed 20,000 loads while atomically replacing
  the key path among matching regular, mismatched regular, and symlinked mismatched material, then
  20,000 equivalent certificate-path loads. Results were key `2795 accepted / 17205 rejected` and
  certificate `912 accepted / 19088 rejected`. Because the non-good alternatives were mismatched
  or symlinked, every success represented a coherent matching snapshot; no unsafe path-race
  acceptance was observed. Probe formatting and Clippy passed, and the complete probe directory was
  removed.
- `cargo test --locked -p xtask unsafe_code_is_confined_to_audited_windows_boundary` passed 1/1,
  and `cargo run --locked -p xtask -- check-support` passed.

### Windows same-handle review

- Static trace confirms one `File` is opened with `FILE_FLAG_OPEN_REPARSE_POINT`; reparse-point
  metadata, owner/DACL queries through `GetKernelObjectSecurity`, and subsequent key reads all use
  that live file/handle rather than re-resolving the pathname.
- The descriptor path fails closed on invalid descriptors, owner mismatch, absent/unprotected DACL,
  non-two-ACE ACLs, inherited/non-allow/non-full-control ACEs, unexpected SIDs, malformed ACE/SID
  lengths, and every Win32 API failure. Allocation sizes and ACE/SID pointer reads are bounded by
  Windows-returned sizes plus checked arithmetic.
- A disposable harness including the exact two FFI source files compiled successfully with Rust
  1.91.0 and pinned `windows-sys = 0.61.2` for `aarch64-pc-windows-msvc`.
- The same harness failed Clippy under the repository's denied policies:
  `undocumented_unsafe_blocks` at `relay_tls_windows.rs:159` for the second `EqualSid` call, and
  `multiple_unsafe_ops_per_block` at line 199 for pointer addition plus dereference in one unsafe
  block. These are policy/audit defects; the static review did not demonstrate memory unsafety or a
  bypass from them.
- Native Windows execution was unavailable on this Linux host. No claim is made that ACL behavior
  was dynamically validated on NTFS or that Windows relay startup succeeded.

### Residual platform risk

- Windows ACL enforcement and reparse behavior still lack fresh native execution at this exact tip.
- The AArch64 MSVC compile probe validates type checking and FFI signatures, not linking or runtime
  behavior.
- The strict Windows unsafe-boundary lint gate is currently red at exact tip, contradicting the
  prior approval evidence and weakening assurance that every unsafe operation is independently
  documented and audited.
- The local race probe exercised Linux rename/symlink semantics only; it does not substitute for a
  Windows concurrent rename/reparse test on NTFS.
