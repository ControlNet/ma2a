# Todo 1 Workspace Bootstrap Knowledge

## Reproducible gate composition

The root `xtask` is the portable authority for exact dependency pins, source-file LOC limits,
the six-target support contract, Rust gates, and web gates. Shell wrappers remain thin entry
points for local use and CI.

The frontend release path is intentionally build-time only: Bun performs a frozen install,
Vite writes ignored `web/dist` assets, and `ma2a-app/build.rs` embeds those files into the
binary. A missing or failed frontend build is therefore a release-build failure rather than a
runtime fallback.

## Tooling caveats

Run `cargo-machete` directly from `xtask` instead of spawning it through nested Cargo. Nested
Cargo inherits package-scoped environment variables and can misidentify the workspace
manifest.

Rust LSP may time out on this dependency graph during bootstrap. The reliable fallback gate is
the combination of locked all-target `cargo check`, strict Clippy, rustfmt, and nextest, while
still recording the LSP limitation explicitly.

## Dependency policy

The exact Iroh baseline is `iroh`, `iroh-base`, and `iroh-relay` 1.1.0 with the required 1.0.x
ancillary crates. `cargo-deny` exceptions must remain narrow and documented; the current two
exceptions are unmaintained transitive crates for which the pinned Iroh graph has no safe
upgrade path.

`RUSTSEC-2024-0436` applies to `paste`; `RUSTSEC-2023-0089` applies to `atomic-polyfill`.
When documenting an advisory, use `deny.toml` and `docs/dependencies.md` as the authoritative
mapping rather than copying transitive crate names from memory.

## Verifier hardening

Workspace path dependencies must be canonicalized and matched against
`cargo metadata.workspace_members`. A lexical `starts_with(workspace_root)` test does not stop
`..` traversal to an external package.

Deferred platform probes are useful only when they record observed facts. The support contract
stores the command, integer exit code, outcome, output, and evidence host, and rejects future
intentions as evidence. Linux-host ARM64 cross-compilation failures describe missing host
toolchains; they do not establish native target support.

For a wholly untracked bootstrap, `git diff --check` is vacuous. Use a temporary index via
`GIT_INDEX_FILE`, stage into that index, and run `git diff --cached --check` so whitespace is
validated without changing the real index. Likewise, secret scanning must include
`git ls-files --cached --others --exclude-standard`, not only tracked files.

After the bootstrap is committed, verify lockfile provenance with `git ls-files` and run
adversarial policy mutations in a disposable `git archive` copy. This proves command-boundary
failures without touching the product worktree. For the Todo 1 frontend, production browser QA
should expect an intentionally empty React mount: prove the title/root, local asset responses,
clean console, zero external requests, and responsive absence of overflow rather than expecting
the later Todo 5 application shell.
