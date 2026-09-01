# Todo 8: Single-Owner Space Manifests

## 2026-08-29 implementation findings

- The inherited WIP already had deterministic canonical CBOR, independent raw golden vectors,
  strict signatures/hashes, linear chain validation, immutable authorization snapshots, and real
  SQLite/KeyStore tests.
- The meaningful missing regression was complete revocation-state continuity: a revoked endpoint
  could disappear from later revocation arrays while remaining absent. `SpaceChain` now retains the
  complete current revocation set and accepts removal only through an explicit valid re-addition.
- Repository reopen now verifies the genesis-derived Space ID, generation-zero row, every stored
  generation/previous-hash/manifest-hash tuple, highest metadata, and materialized member/revocation
  rows against the signed chain.
- Repository-owned advancement loads the protected authority seed through `KeyStore`, keeps the
  loaded bytes in a zeroizing buffer, verifies the public authority, signs only the exact next link,
  commits chain plus Runtime revision, and derives authorization after commit.
- Legacy raw Space and manifest persistence methods were retained for neighboring Todo 3 callers but
  hardened to parse and verify signed objects before mutation. Independent member/revocation writes
  were removed so signed chain persistence is the sole authority for those materialized tables.
- Rust LSP diagnostics remain unavailable for `/tmp/opencode/ma2a-todo-8` because the tool rejects
  paths outside the request cwd before rust-analyzer starts.

## 2026-08-29 independent review remediation

- A failing regression proved that an Endpoint key could substitute for the Space authority key;
  genesis now rejects equality between the authority and initial-member Endpoint public keys.
- A failing regression proved that `import_public` applied the 32,768-byte object limit to an entire
  portable chain; aggregate bounds now permit 255 signed manifests up to 8,389,381 encoded bytes.
- A failing store regression proved identical signed replay was reported as `Conflict`; the
  repository now returns `ManifestOutcome::Idempotent` without changing the revision.
- The shared store fixture was corrected to use distinct deterministic Endpoint and authority keys.
- A disposable revocation-continuity mutation failed the intended adversarial assertion and passed
  again after exact restoration.
- Final gates passed: 43 Core tests, 25 Store tests, 91 workspace tests, strict Clippy, rustfmt,
  `cargo xtask check`, LOC, dependency checks, 41 web tests, OpenSSL signature verification, and
  standalone BLAKE3 vector verification.
- Review remediation was committed as `7e2e8e1`, `96d56d2`, `4706879`, and `6f72a04`; `.omo/**`
  review artifacts remain deliberately untracked.
