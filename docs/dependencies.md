# Dependency Inventory

MA2A resolves dependencies once, commits both lockfiles, and requires exact direct version
requirements, in `[patch]` tables and in every dependency section of `web/package.json` too; a
git source and any `[replace]` entry are refused. Run `cargo xtask check-pins` after changing any manifest.

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
| `iroh-dns` | 1.1.0 | DNS resolver for relay tests |
| `iroh-tickets` | 1.0.0 | Iroh-compatible ticket encoding |
| `n0-future` | 0.3.2 | Address lookup streams |
| `tokio` | 1.53.3 | Async runtime |
| `tokio-util` | 0.7.19 | Structured Runtime cancellation tokens |
| `tokio-stream` | 0.1.19 | Event streams for the Web UI and A2A adapter |
| `axum` | 0.8.9 | Loopback HTTP surface |
| `tower` | 0.5.3 with `util` | HTTP service composition |
| `hyper-util` | 0.1.20 with `server-auto`, `service`, and `tokio` | Bounded HTTP connection serving |
| `http` | 1.5.0 | HTTP types |
| `rusqlite` | 0.41.1 with `backup`, `bundled`, and `fallible_uint` | Embedded SQLite state, checked counters, and online backup |
| `rustix` | 1.1.6 with `fs`, `process`, `std`, and `termios` | Unix file, ownership, and terminal calls |
| `portable-pty` | 0.9.0 | Shell service terminals |
| `zeroize` | 1.9.0 with `alloc` | Zeroizing protected-key buffers |
| `ed25519-dalek` | 3.0.0 | Space authority signatures |
| `blake3` | 1.8.7 | Domain-separated `SpaceId` derivation and content digests |
| `getrandom` | 0.4.3 | Operating-system randomness for `RequestId` |
| `argon2` | 0.6.0 with `alloc`, `getrandom`, `password-hash`, and `zeroize` | Web UI password verifier |
| `subtle` | 2.6.1 | Constant-time secret comparison |
| `rustls`, `tokio-rustls` | 0.23.46 with `ring` and `std`; 0.26.4 with `ring` and `tls12` | Private relay TLS |
| `rustls-pki-types` | 1.15.1 | Private relay certificate and key types |
| `x509-parser` | 0.18.1 | Private relay certificate checks |
| `rcgen` | 0.14.9 | Test certificates for relay TLS |
| `interprocess` | 2.4.3 with `tokio` | Cross-platform local IPC |
| `widestring`, `windows-sys` | 1.2.1, 0.61.2 | Windows security and file APIs |
| `clap` | 4.6.6 with `derive` | CLI parsing |
| `rpassword` | 7.5.4 | Terminal password and invite ticket input |
| `serde`, `serde_json` | 1.0.229, 1.0.151 | JSON for the local API, Web UI, A2A adapter, and `xtask` policies |
| `base64` | 0.22.1 | Artifact content in JSON results |
| `sha2` | 0.11.0 | Test digests |
| `proptest` | 1.11.0 with `std` only | Core wire and ontology property tests |
| `toml` | 1.1.4 | `xtask` Cargo manifest parsing |

The eight product crates (`ma2a-agent`, `ma2a-app`, `ma2a-core`, `ma2a-net`, `ma2a-runtime`,
`ma2a-service`, `ma2a-services`, `ma2a-store`) use exact `=0.1.0` path requirements. `xtask` is a non-shipping
workspace utility. `Cargo.lock` is authoritative for the complete transitive Rust inventory;
`web/bun.lock` is authoritative for the complete frontend inventory.

`cargo-deny` temporarily acknowledges `RUSTSEC-2023-0089` (`atomic-polyfill`) and
`RUSTSEC-2024-0436` (`paste`). Both are unmaintained, not vulnerability advisories, have no safe
upgrade in the required Iroh 1.1.0 graph, and must be re-evaluated when that pinned family moves.
`RUSTSEC-2026-0543` (`chrono`, unmaintained, no vulnerability) is acknowledged on the same terms:
it arrives through iroh-relay's ACME client and, on Windows, through `netwatch`/`wmi`. Iroh 1.5.0
would remove the ACME path but not the Windows one. Review it by 2027-05-20.

## Frontend Direct Dependencies

| Dependency | Exact version | Purpose |
| --- | --- | --- |
| `@fontsource-variable/space-grotesk` | 5.3.0 | Self-hosted UI typeface, latin subset only (OFL-1.1) |
| `@fontsource-variable/jetbrains-mono` | 5.3.0 | Self-hosted identifier typeface, latin subset only (OFL-1.1) |
| `ky` | 2.1.0 | Same-origin HTTP client boundary |
| `react` | 19.2.8 | Component runtime |
| `react-dom` | 19.2.8 | Browser DOM renderer |
| `zod` | 4.5.1 | Runtime snapshot and event boundary parsing |
| `@biomejs/biome` | 2.5.11 | Formatter and static analysis |
| `@tailwindcss/vite` | 4.3.3 | Tailwind integration for Vite |
| `@testing-library/jest-dom` | 7.0.1 | DOM-specific test assertions |
| `@testing-library/react` | 16.3.3 | React component test rendering |
| `@testing-library/user-event` | 14.6.6 | Keyboard and pointer interaction tests |
| `@types/bun` | 1.3.5 | Bun runtime types |
| `@types/jsdom` | 30.0.0 | JSDOM TypeScript declarations |
| `@types/react` | 19.2.18 | React TypeScript declarations |
| `@types/react-dom` | 19.2.5 | React DOM TypeScript declarations |
| `@vitejs/plugin-react` | 6.1.1 | React transform and refresh integration |
| `@vitest/coverage-v8` | 4.1.11 | V8 coverage reporting for Vitest |
| `axe-core` | 4.13.0 | Automated route accessibility assertions |
| `jsdom` | 30.0.1 | Browser-like DOM for component tests |
| `react-doctor` | 0.9.12 | Development-only React diagnostics |
| `react-grab` | 0.2.0 | Development-only component inspection |
| `react-scan` | 0.5.7 | Development-only render diagnostics |
| `tailwindcss` | 4.3.3 | CSS utility compiler |
| `typescript` | 5.9.3 | Strict frontend type checking |
| `vite` | 8.2.2 | Frontend development and production build |
| `vitest` | 4.1.11 | Frontend test runner |

Both font packages are vendored into the bundle at build time. `src/styles/fonts.css`
declares only the latin `woff2` face of each family, so the embedded binary carries two font
files rather than the eleven the packages ship. No stylesheet may reference a remote origin;
`src/styles/fonts.test.ts` enforces that, and the release smoke test re-proves it by fetching
every served asset and rejecting any external reference.

## Update Procedure

1. Change one direct requirement to an exact version.
2. Regenerate the relevant lockfile with Cargo or Bun; never hand-edit a lockfile.
3. Run `cargo xtask check-pins`, `cargo tree -d`, `cargo deny check`, and `cargo machete`.
4. Run `scripts/check.sh` and update this inventory when the direct baseline changes.
