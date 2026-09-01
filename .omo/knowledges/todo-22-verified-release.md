# Todo 22 Verified Release Pipeline

## Release contract

- Cargo-dist is pinned to `0.32.0` and configured through `dist-workspace.toml`.
- Supported release targets are exactly the `supported` entries in `docs/platform-support.json`:
  - `x86_64-unknown-linux-gnu`
  - `x86_64-apple-darwin`
  - `x86_64-pc-windows-msvc`
- ARM64 Linux, macOS, and Windows remain deferred until native build and smoke evidence exists.
- Unix releases use `.tar.xz`; Windows releases use `.zip`; each archive has a SHA-256 sidecar.
- Each archive contains one cargo-dist top-level directory and exactly the executable, `README.md`, `quickstart.md`, `LICENSE-MIT`, `LICENSE-APACHE`, and `NOTICE`.
- Installers, source tarballs, cargo-dist CI generation, and cargo-dist hosting are disabled.

## Policy and automation

- `cargo xtask check-release` validates cargo-dist version, target matrix equality, archive formats, checksum policy, disabled outputs, and exact archive includes.
- `cargo xtask dist` validates both the release policy and that the current host target is supported before invoking cargo-dist.
- Clean-room password automation is opt-in through `MA2A_PASSWORD_STDIN=1`; secrets are read as newline-delimited stdin and never supplied through argv or URLs.
- `.github/workflows/release.yml` derives its build matrix from supported targets, validates archives, smokes extracted binaries, emits SPDX and license metadata, creates provenance and SBOM attestations, preserves Sigstore bundles, runs negative verification probes, and publishes archives only for tag runs.
- `actions/attest@v4` requires `id-token: write`, `attestations: write`, and `artifact-metadata: write`; its `bundle-path` output contains the generated Sigstore bundle.
- SPDX attestation verification uses predicate type `https://spdx.dev/Document/v2.3`.
- Derive SBOM creation time from `git show -s --format=%cI "$GITHUB_SHA"`; `github.event.head_commit.timestamp` is not reliable for tag and manual events.

## Local verification evidence

- `cargo clippy --locked -p xtask --all-targets -- -D warnings`
- `cargo test --locked -p xtask release_`: 5 passed.
- `cargo run --locked -p xtask -- check-release`
- `cargo run --locked -p xtask -- check-support`
- `cargo run --locked -p xtask -- check-pins`
- `cargo run --locked -p xtask -- dist` built the native Linux archive through the real local entry point.
- `scripts/validate-release.sh` confirmed the exact archive payload and checksum.
- License report and SPDX generation plus `scripts/validate-release-metadata.sh` passed.
- `scripts/smoke-release.sh` passed version, daemon auto-start, stdin password setup, Space creation, loopback Web URL, embedded HTML/JavaScript fetch, and graceful shutdown using the extracted binary.
- `scripts/test-release-failures.sh` rejected a tampered archive, a missing quickstart asset, and an incomplete frontend SBOM.
- Bun parsed `.github/workflows/release.yml`; `actionlint` was not installed locally.
- Rust LSP requests timed out repeatedly; compiler, clippy, tests, and real artifact smoke supplied validation instead.

## Environment limitations

- Native Windows, Apple, and ARM64 Linux build/smoke execution was unavailable on the Linux x86_64 host.
- GitHub OIDC and attestation API behavior cannot be executed locally.
- `cargo-cyclonedx`, `syft`, and `pwsh` were unavailable; the repository-owned SPDX generator was used.
