# Build and Verification

## Prerequisites

- Rust 1.91.0 installed through `rustup`; the repository toolchain file selects it.
- Bun 1.3.5.
- `cargo-nextest`, `cargo-deny`, and `cargo-machete` for the complete local gate.

Install the Cargo tools with:

```sh
cargo install --locked cargo-nextest@0.9.143 cargo-deny@0.20.2 cargo-machete@0.9.2
```

## Commands

```sh
./scripts/check.sh
./scripts/test.sh
./scripts/web-build.sh
./scripts/dist.sh
```

`cargo dist` validates the release policy and invokes pinned cargo-dist 0.32.0 for the current host
target. The app build script performs the frozen frontend install/build and embeds every file from
`web/dist`; generated assets remain ignored and are never runtime source-tree dependencies. The
archive and checksum are written below `target/distrib`.

The exact six-target architecture and evidence contract is checked by `cargo xtask check-support`
against `docs/platform-support.json`; `check-release` composes that gate with cargo-dist policy.
Release CI runs `check-support` before deriving native x86_64 jobs for Linux, macOS, and Windows.
ARM64 targets remain evidenced as deferred until native/QEMU smoke runners are available. Tag builds
then derive their release matrix from the validated targets marked `supported` and require archive,
checksum, authenticated clean-room smoke, offline embedded-asset traversal, SPDX SBOM,
dependency/license, and provenance gates before publication.
