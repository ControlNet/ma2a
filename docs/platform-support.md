# Platform Support

The release workflow builds every target marked `supported` in `platform-support.json`. A target is
not supported until compile, archive packaging, and clean extracted-binary smoke all have a reliable
native or emulated path.

| Target | Status | Evidence path |
|---|---|---|
| `x86_64-unknown-linux-gnu` | Supported | Native Ubuntu build and clean-room smoke |
| `x86_64-apple-darwin` | Supported | Native `macos-15-intel` build and clean-room smoke |
| `x86_64-pc-windows-msvc` | Supported | Native `windows-latest` build and clean-room smoke |
| `aarch64-unknown-linux-gnu` | Deferred | Linux evidence host lacks `aarch64-linux-gnu-gcc` and `qemu-aarch64` |
| `aarch64-apple-darwin` | Deferred | Linux evidence host lacks an Apple SDK/toolchain; rerun on native Apple ARM64 |
| `aarch64-pc-windows-msvc` | Deferred | No Windows ARM64 MSVC toolchain plus reliable native/emulated smoke runner |

## Reproduce Deferred Probes

```sh
cargo check --locked --workspace --all-targets --all-features --target TARGET
cargo build --locked --release -p ma2a-app --target TARGET
qemu-aarch64 target/aarch64-unknown-linux-gnu/release/ma2a --version
```

For Apple and Windows use the target binary directly on the matching native runner. The exact exit
codes and captured blocker output are in `platform-support.json`. Linux ARM64 is re-evaluated after
installing the GNU cross-compiler and QEMU. Apple ARM64 is re-evaluated on a native Apple ARM64
runner. Windows ARM64 is re-evaluated only when both packaging and reliable execution are available.

`cargo xtask check-support` requires this exact six-target set and rejects every entry without
supported evidence or a concrete deferred blocker and re-evaluation condition. `cargo xtask
check-release` runs that support check first, then rejects cargo-dist targets that differ from the
validated supported subset. Release CI runs `check-support` before deriving its build matrix, so a
missing supported or deferred architecture cannot silently remove a release job.
