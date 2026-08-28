# Dependency Inventory

MA2A resolves dependencies once, commits both lockfiles, and requires exact direct version
requirements. Run `cargo xtask check-pins` after changing any manifest.

## Toolchains

| Tool | Version source | Pinned version |
| --- | --- | --- |
| Rust | `rust-toolchain.toml` | 1.91.0 |
| Rust edition | workspace manifest | 2024 |
| Bun | `web/package.json` | 1.3.5 |
| TypeScript | `web/package.json` | 5.9.3 |

## Rust Direct Dependencies

| Dependency | Exact requirement | Purpose |
| --- | --- | --- |
| `iroh`, `iroh-base`, `iroh-relay` | 1.1.0 | Endpoint transport and relay primitives |
| `iroh-tickets` | 1.0.0 | Iroh-compatible ticket encoding |
| `iroh-ping` | 1.0.0 | Narrow connectivity probe |
| `iroh-metrics` | 1.0.1 | Transport metrics types |
| `irpc`, `irpc-iroh` | 0.17.0 | Typed RPC composition over Iroh |
| `tokio` | 1.53.1 | Async runtime |
| `axum` | 0.8.9 | Loopback HTTP surface |
| `rusqlite` | 0.40.2 with `bundled` | Embedded SQLite state |
| `ed25519-dalek` | 3.0.0 | Space authority signatures |
| `blake3` | 1.8.7 | Domain-separated `SpaceId` derivation |
| `getrandom` | 0.4.3 | Operating-system randomness for `RequestId` |
| `proptest` | 1.11.0 with `std` only | Core wire and ontology property tests |
| `interprocess` | 2.4.3 | Cross-platform local IPC |
| `serde_json` | 1.0.151 | `xtask` JSON policy parsing |
| `toml` | 1.1.4 | `xtask` Cargo manifest parsing |

The five product crates use exact `=0.1.0` path requirements. `xtask` is a non-shipping
workspace utility. `Cargo.lock` is authoritative for the complete transitive Rust inventory;
`web/bun.lock` is authoritative for the complete frontend inventory.

`cargo-deny` temporarily acknowledges `RUSTSEC-2023-0089` (`atomic-polyfill`) and
`RUSTSEC-2024-0436` (`paste`). Both are unmaintained, not vulnerability advisories, have no safe
upgrade in the required Iroh 1.1.0 graph, and must be re-evaluated when that pinned family moves.

## Frontend Direct Dependencies

| Dependency | Exact version | Purpose |
| --- | --- | --- |
| `react` | 19.2.8 | Component runtime |
| `react-dom` | 19.2.8 | Browser DOM renderer |
| `@biomejs/biome` | 2.5.11 | Formatter and static analysis |
| `@tailwindcss/vite` | 4.3.3 | Tailwind integration for Vite |
| `@types/bun` | 1.3.5 | Bun runtime types |
| `@types/react` | 19.2.18 | React TypeScript declarations |
| `@types/react-dom` | 19.2.5 | React DOM TypeScript declarations |
| `@vitejs/plugin-react` | 6.1.1 | React transform and refresh integration |
| `react-doctor` | 0.9.12 | Development-only React diagnostics |
| `react-grab` | 0.2.0 | Development-only component inspection |
| `react-scan` | 0.5.7 | Development-only render diagnostics |
| `tailwindcss` | 4.3.3 | CSS utility compiler |
| `typescript` | 5.9.3 | Strict frontend type checking |
| `vite` | 8.2.2 | Frontend development and production build |
| `vitest` | 4.1.11 | Frontend test runner |

## Update Procedure

1. Change one direct requirement to an exact version.
2. Regenerate the relevant lockfile with Cargo or Bun; never hand-edit a lockfile.
3. Run `cargo xtask check-pins`, `cargo tree -d`, `cargo deny check`, and `cargo machete`.
4. Run `scripts/check.sh` and update this inventory when the direct baseline changes.
