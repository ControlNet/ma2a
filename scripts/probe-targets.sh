#!/usr/bin/env bash
set -euo pipefail

target=${1:?usage: probe-targets.sh TARGET [OUTPUT_DIR]}
output=${2:-target/architecture-evidence/$target}
mkdir -p "$output"

set +e
cargo check --locked --workspace --all-targets --all-features --target "$target" >"$output/compile.log" 2>&1
compile_exit=$?
cargo build --locked --profile dist -p ma2a-app --target "$target" >"$output/package.log" 2>&1
package_exit=$?
case "$target" in
  aarch64-unknown-linux-gnu)
    qemu-aarch64 "target/$target/dist/ma2a" --version >"$output/smoke.log" 2>&1
    ;;
  *-pc-windows-msvc)
    "target/$target/dist/ma2a.exe" --version >"$output/smoke.log" 2>&1
    ;;
  *)
    "target/$target/dist/ma2a" --version >"$output/smoke.log" 2>&1
    ;;
esac
smoke_exit=$?
set -e

printf 'target=%s\ncompile_exit=%s\npackage_exit=%s\nsmoke_exit=%s\n' \
  "$target" "$compile_exit" "$package_exit" "$smoke_exit" > "$output/results.txt"
cat "$output/results.txt"
