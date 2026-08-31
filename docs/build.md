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

The architecture contract is checked by `cargo xtask check-support` against
`docs/platform-support.json`. CI runs native x86_64 jobs for Linux, macOS, and Windows and
records non-blocking ARM64 compile probes until native/QEMU smoke runners are available. Tag builds
derive their release matrix directly from the targets marked `supported` and require archive,
checksum, clean-room smoke, SPDX SBOM, dependency/license, and provenance gates before publication.
