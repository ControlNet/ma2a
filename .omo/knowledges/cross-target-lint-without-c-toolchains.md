# Cross-target lint of pure-Rust crates from a Linux host

The Linux dev host has the `aarch64-apple-darwin` and `aarch64-pc-windows-msvc` Rust targets but no
Apple or MSVC C toolchains. `blake3`'s build script compiles C/assembly SIMD kernels and fails there
(`-arch` unrecognized, `arm_neon.h` missing). Crates that depend on `blake3` directly can still be
linted per target with blake3's pure-Rust backend:

```sh
for t in aarch64-apple-darwin aarch64-pc-windows-msvc; do
  cargo clippy --color never -p ma2a-services --all-targets --target "$t" \
    --features blake3/pure -- -D warnings
done
```

This catches `cfg(windows)`/`cfg(unix)` API mistakes and platform-only lints (for example
`missing_const_for_fn` firing only on the branch where a body becomes empty, or rustix functions
such as `mknodat` that do not exist on Apple). Crates pulling `ring` (ma2a-net, ma2a-runtime) still
need native CI for these targets, as recorded in `github-actions-platform-fixes.md`.
