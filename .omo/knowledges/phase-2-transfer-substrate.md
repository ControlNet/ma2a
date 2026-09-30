# Phase 2 Transfer substrate: native MA2A QUIC streaming, not iroh-blobs

Investigated 2026-10-01 against the pinned stack (iroh 1.1.0, tokio 1.53.1, Rust 1.91.0).

## iroh-blobs facts

- Latest `iroh-blobs` 0.103.0 (MIT OR Apache-2.0, MSRV 1.91) requires `iroh ^1.0.0` and resolves
  with `iroh =1.1.0`. Still 0.x with frequent minor releases.
- With default features and tokio pinned exactly, it fails to compile (`block_in_place`,
  `new_multi_thread`) because it uses tokio `rt-multi-thread` without declaring it.
- Serving: `BlobsProtocol::new(&store, Option<EventSender>)` on ALPN `/iroh-bytes/4`.
  Authorization hooks are asynchronous event interceptions (`EventMask{connected, get: Intercept}`)
  answered with `AbortReason::Permission`; `GetRequestReceived` carries only a `connection_id`, so
  per-EndpointId checks need our own connection→Endpoint map.
- Storage must be iroh-blobs' own `MemStore` (unbounded RAM) or `FsStore` (redb + files).
  `FsStore` creates its own multi-thread tokio runtime sized to CPU count. No quotas; each incoming
  stream is spawned without a bound other than QUIC stream limits.
- Resume: bao-verified range requests (its main strength); across restart only with `FsStore`.
- Weight: +30–40 crates (redb, rcgen, chrono, irpc, genawaiter, syn v1, proc-macro-error).
  `cargo deny` fails on RUSTSEC-2024-0370 (proc-macro-error unmaintained).

## Decision

| Axis | Native MA2A | iroh-blobs |
| --- | --- | --- |
| Authorization integration | inline single-Space decision before any body byte | async hook + side map |
| Endpoint targeting | yes | yes |
| Bounded resources | one 64 KiB frame, fixed permits | no quotas, hidden runtime |
| Resume | offset + content identity + final BLAKE3 | built in |
| Dependency complexity | none new (blake3 already pinned) | 30–40 crates, advisory ignore |
| Protocol control | full, versioned with MA2A | upstream ALPN/wire format |

Native streaming wins. A content-addressed substrate (bao-tree directly) can be revisited when
Phase 4 artifacts need verified ranged reads by hash; the `TransferResource` enum is the extension
point.

## PTY choice (Shell)

`portable-pty` 0.9.0 (MIT): Linux/macOS PTY and Windows ConPTY with resize in one crate; blocking
I/O bridged with `spawn_blocking`. Adds nix 0.28 on Unix (no duplicate), no rustix conflict.
`pty-process` 0.5.3 is async and rustix-based but Unix-only.
