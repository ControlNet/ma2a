# ma2a-phase-2 — Remote Capability Layer

Baseline: Phase 1 freeze `06a9bad5da23cefb9941a824aaa34ff21300327a` (frozen infrastructure).

## Goal

Turn MA2A from an authenticated Echo network into a remote-computing fabric with five closed,
built-in capability families: **Files, Transfer, Exec, Shell, Proxy**. No persistent Process
objects, no Agent runtime, no generic plugin/service framework, no dynamic registration.

## Architecture decisions

### D1 — Caller authority: signed member capability bits

Space member records gain five caller capabilities, signed by the Space authority in the next
manifest generation:

| Bit | Value | Capability | Meaning in one Space |
| --- | --- | --- | --- |
| 0 | 1 | `echo` | (Phase 1) |
| 1 | 2 | `private_relay_provider` | (Phase 1) |
| 2 | 4 | `files_read` | may stat/list/read/download from members that export a root |
| 3 | 8 | `files_write` | may mkdir/write/rename/remove/upload into read-write roots |
| 4 | 16 | `exec` | may run one-shot Exec on members that expose Exec |
| 5 | 32 | `shell` | may open interactive Shell on members that expose Shell |
| 6 | 64 | `proxy` | may open TCP Proxy streams to destinations a member allows |

`MemberCapabilities` widens internally to `u16` so Phase 3/4 bits fit; the CBOR encoding is an
unsigned integer, so Phase 1 manifests (bits ≤ 3) decode byte-identically and the golden vector is
unchanged. Remote-service bits are governed only by signed member grants: the Phase 1 genesis policy
(`echo`, `private_relay_provider`, `maximum_members`) is not extended, because a new signed policy
key would make every pre-Phase-2 Space permanently unable to adopt remote services while adding no
authority beyond the single owner's per-member signature. Grants default off, so no existing Space
or enrollee gains a remote capability without an explicit owner action (`space_grant`).

### D2 — Target consent: per-Space local exposure (durable desired state)

A signed grant says what a caller may ask for; it never makes a target serve anything. Each Runtime
keeps a local, per-Space **exposure set** in SQLite:

- `files_root {name, absolute canonical path, access: read_only | read_write}`
- `exec` (enabled)
- `shell` (enabled)
- `proxy_rule {host: exact DNS name | IP literal | "*", port_start..=port_end}`

Exposure is scoped to one Space so that joining a Space owned by someone else can never hand that
authority remote execution or file access: the local operator decides per Space.

**Authorization rule (single complete Space).** A Phase 2 request is permitted iff one Space `S`
exists such that: caller ∈ S and target ∈ S (current, unrevoked); caller's signed member
capabilities in S include the operation's capability; and the target's local exposure *for S*
covers the requested resource (root + access, exec, shell, proxy destination). Every condition is
evaluated against the same `S`; a grant in S1 and an exposure in S2 never combine. The chosen Space
is never returned to the caller. Every failure is the same uniform `unauthorized`.

Authorization runs twice on each stream: a coarse **admission** (some single Space where the caller
holds this family's capability and the target exposes this family) before any byte of the stream is
read, and the complete **resource decision** after the bounded Open header (≤ 16 KiB) is decoded and
before any body/stdin/proxy byte is read. Exposure/grant state is read from the Store at each
decision (no cache), so revocation and unexposure affect the next stream immediately.

### D3 — Transport layout

One exact ALPN per family, mirroring Phase 1: `ma2a/files/1`, `ma2a/transfer/1`, `ma2a/exec/1`,
`ma2a/shell/1`, `ma2a/proxy/1`. They are negotiated only while the Runtime has ≥ 1 Space (zero-Space
Endpoints still advertise only enrollment). They are advertised regardless of exposure so ALPN
negotiation never reveals local exposure state. One operation = one QUIC bidirectional stream.

### D4 — Service stream framing v1 (shared by all families and by the local stream plane)

`type:u8 || length:u32 BE || payload`. Header frames ≤ 16 KiB, data frames ≤ 64 KiB payload.

| Type | Name | Direction | Payload |
| --- | --- | --- | --- |
| 1 | `open` | client→server, first frame | canonical CBOR request header |
| 2 | `accept` | server→client | canonical CBOR accept header |
| 3 | `reject` | server→client | status code (u8); guarantees no side effect happened |
| 4 | `data` | both | channel:u8 || bytes |
| 5 | `end` | both | channel:u8 (logical half-close) |
| 6 | `control` | both | canonical CBOR (resize, commit, …) |
| 7 | `outcome` | server→client, terminal | canonical CBOR terminal summary |
| 8 | `cancel` | client→server | empty |

QUIC flow control provides backpressure end to end; every buffer is one frame. A stream reset or
connection loss is treated as `cancel`. `reject` is emitted only before any effect; after `accept` the
only terminal is `outcome` (or loss, which is indeterminate to the client).

### D5 — Local stream plane (control/state API vs high-volume streams)

The CLI never owns Endpoint identity. For every remote Phase 2 operation it opens a dedicated IPC
connection and sends `stream_open {request_id, target_endpoint_id, service}` (IPC-only, not an HTTP
route). After `stream_opened`, the connection carries D4 frames verbatim in both directions; the
daemon validates the `open` header (service, target, request id, bounds), dials the target on the
family ALPN, and relays frames with one-frame buffers. Stream connections use their own admission
pool (64) and never hold the business replay lock or a business connection slot while streaming.
The JSON local API gains only control/state commands (grants, exposures, operation status).

### D6 — Replay for side-effecting streams

Mutating operations (files mkdir/write/rename/remove, upload, exec, shell, proxy) reserve their
RequestId in the Phase 1 durable mutation-replay table (fingerprint = SHA-256 of service ‖ target ‖
canonical open header). Terminal handling reuses Phase 1 effect classes:

- dial failure before `open` was written, or remote `reject` → **not started**, reservation released;
- `outcome` received → **completed**; replay returns `stream_replayed {outcome}` without re-execution;
- stream lost after `open` was written and before `outcome` → **indeterminate**, permanent terminal
  error with the Phase 1 "inspect state" remediation.

Same RequestId + different open header → `conflict`. Read-only operations (stat, list, read-small,
download) do not reserve (Phase 1 reads carry no replay). No remote-side replay table is created:
the daemon never semantically retries, which is the Phase 1 Echo contract.

### D7 — Files semantics

- **Namespace:** `ROOT:/a/b`. Wire form: root name (1–64 bytes, `[a-z0-9][a-z0-9._-]*`) + ordered
  component list (≤ 64 components, each 1–255 UTF-8 bytes, total ≤ 4,096). Components `.`/`..`,
  empty, NUL, `/`, `\`, `:` and control characters are invalid; on Windows hosts, reserved device
  names and trailing dot/space are also invalid. The CLI collapses repeated/trailing `/`; nothing is
  lexically resolved, so every accepted path names exactly one root-relative location.
- **Roots:** absolute existing directories, canonicalized when exposed.
- **Symlinks:** never followed. An intermediate symlink component → `symlink_refused`. A final
  symlink is reported as `symlink` by stat/list (target never disclosed), may be removed or renamed
  as a link, and refuses read/write. Remote callers cannot create symlinks. Containment is per
  component `lstat`; concurrent modification by other same-user local processes is outside the threat
  model and documented.
- **Metadata:** kind (file/directory/symlink/other), size, modified time (ms), read-only flag.
- **list:** byte-order sorted, ≤ 512 entries per page, opaque cursor (last name).
- **mkdir:** `parents`; an existing directory with `parents` is success, otherwise `already_exists`.
- **read-small:** offset + length ≤ 65,536 bytes. **write-small:** ≤ 65,536 bytes; modes
  `create_new` (atomic no-clobber via temp + hard link) and `replace` (temp + fsync + atomic rename).
- **rename:** same root, `replace` flag; no-replace uses the platform's atomic no-clobber rename.
- **remove:** file/symlink or empty directory; `recursive` removes a tree within the deadline.
- **errors:** `not_found`, `already_exists`, `not_a_directory`, `is_a_directory`, `not_empty`,
  `symlink_refused`, `invalid_path`, `permission_denied`, `read_only_root`, `too_large`, `io`.
- Larger content always uses Transfer; the ordinary 64 KiB bound is never raised.

### D8 — Transfer (native MA2A QUIC streaming; iroh-blobs rejected)

Investigation (`iroh-blobs` 0.103.0 vs native) is recorded in
`.omo/knowledges/phase-2-transfer-substrate.md`. Native wins on authorization integration, bounded
resources, dependency weight and protocol control; resume is implemented natively.

- **Resource model:** `TransferResource::File {root, path}` in Phase 2 (typed, extensible for
  Process logs / Agent artifacts later). Authorization maps download → `files_read`, upload →
  `files_write` on a `read_write` root.
- **Download:** `open {resource, offset, expected_identity?}` → `accept {size, identity}` → `data`
  chunks → `outcome {blake3(full content), bytes}`. Identity = size + modified-ns (+ file id where
  available); the server hashes the full content (including any skipped prefix) and fails with
  `content_changed` if identity changes during the stream. The client verifies BLAKE3 over its final
  file. **Resume:** the client keeps `NAME.ma2a-partial` plus a JSON sidecar (source, identity) and
  resumes with `offset` + `expected_identity`; mismatch → `precondition_failed`, client restarts.
  Resume survives any daemon restart because every fact it needs is persisted client-side and the
  final hash proves the result.
- **Upload:** `open {resource, size, mode}` → server persists a staging record in SQLite *before*
  creating `.ma2a-upload-<request_id>.part` in the destination directory → `accept` → `data` →
  client `end` + `control {commit: blake3}` → server checks size + hash, fsyncs, finalizes atomically
  (`create_new`/`replace`), removes the staging record → `outcome`. Cancel/failure removes the staging
  file and record; startup sweeps recorded staging files. **Upload resume is not supported** in
  Phase 2 (documented).
- Bounded memory (one 64 KiB frame), QUIC backpressure, progress from byte counters, cancellation by
  `cancel`/reset/loss.

### D9 — Exec (one-shot, typed argv)

`open {program, argv[], cwd?, env[], stdin: none|pipe, timeout_ms}`. `program` is a bare name
resolved through the target Runtime's `PATH`, or an absolute path; `cwd` absolute or omitted (target
home directory). ≤ 256 argv entries, ≤ 64 env additions (`[A-Za-z_][A-Za-z0-9_]*`), 16 KiB header.
Timeout 1 ms–24 h (CLI default 10 min). Streams: stdin = data ch 0 (+ `end`), stdout ch 1, stderr
ch 2. `outcome {exit_code | signal, reason, duration_ms, stdout_bytes, stderr_bytes}` with reason
`exited | timed_out | cancelled | shutdown | spawn_failed`. Unix children run in their own process
group, killed as a group; on Windows the child is killed (tree kill documented as a limitation).
Client cancel, network loss, timeout and Runtime shutdown all kill the child; nothing detaches.

### D10 — Shell (interactive PTY/ConPTY, ephemeral)

`portable-pty` 0.9.0 (Linux/macOS PTY, Windows ConPTY). `open {program?, argv[], cwd?, env[], term,
cols, rows, max_lifetime_ms}`; default program = `$SHELL` or `/bin/sh` (Unix), `%COMSPEC%` (Windows).
Frames: stdin data ch 0, terminal output data ch 1, `control {resize}`, `end` ch 0 (no more input;
session continues). Disconnect/cancel/shutdown kills the session. Sessions never survive restart.

### D11 — Proxy (Endpoint-targeted TCP byte streams)

`open {host, port, idle_timeout_ms}` → target-side rule check (host matched as requested, not as
resolved; DNS rebinding therefore stays within an operator-approved name) → TCP connect (10 s) →
`accept` or `reject` (`connection_refused`, `unreachable`, `timed_out`, `dns_failed`). Bidirectional
data ch 0, `end` ↔ TCP `shutdown(Write)`, idle timeout, 24 h lifetime. CLI: `proxy forward LISTEN`
maps each accepted local TCP connection to one Proxy stream; `proxy connect` bridges stdio.

### D12 — Limits

| Family | Per peer | Global | Deadline |
| --- | --- | --- | --- |
| Files | 16 | 64 | 15 s per operation |
| Transfer | 4 | 16 | 30 s stall (no progress) |
| Exec | 8 | 32 | request timeout ≤ 24 h |
| Shell | 4 | 16 | lifetime ≤ 24 h |
| Proxy | 64 | 256 | idle ≤ 1 h, lifetime 24 h |
| Local stream plane | — | 64 connections | — |

Permits are RAII and released on every exit path; metrics expose live counts.

### D13 — Audit and visibility

Bounded in-memory operation audit (256): request id, peer, family, operation kind, direction, result
class, bytes in/out, duration. Paths, argv, env, file content, terminal and proxy bytes are
structurally absent. `operations_status` returns active operations (in-memory, empty after restart)
and recent audit; the Web console shows exposures, active operations and per-member grants.

## Local API additions (v1 schema, coordinated Runtime + frontend update)

`space_grant`, `service_expose`, `service_unexpose`, `service_exposure_list`, `operations_status`,
`stream_open` (IPC only). `space_member` gains the five new grant booleans.

## CLI

```text
ma2a space grant SPACE_REF ENDPOINT --capabilities echo,files-read,files-write,exec,shell,proxy
ma2a service expose SPACE_REF files ROOT PATH [--read-only]
ma2a service expose SPACE_REF exec|shell
ma2a service expose SPACE_REF proxy HOST:PORT[-PORT]
ma2a service unexpose SPACE_REF …        ma2a service list [--json]
ma2a ops [--json]
ma2a file stat|ls|mkdir|rm|mv|cat|write ENDPOINT ROOT:/path …
ma2a file get ENDPOINT ROOT:/path LOCAL [--resume]     ma2a file put LOCAL ENDPOINT ROOT:/path
ma2a exec ENDPOINT [--cwd D] [--env K=V] [--timeout T] [--stdin] -- PROGRAM ARGS…
ma2a shell ENDPOINT [--cwd D] [-- PROGRAM ARGS…]
ma2a proxy forward LISTEN_ADDR ENDPOINT HOST:PORT      ma2a proxy connect ENDPOINT HOST:PORT
```

## Crate layout

- `ma2a-core/src/service/*`: capability bits, exposure/authorization types, frames, all message
  codecs, remote path model (sync, canonical CBOR).
- `ma2a-net/src/service_protocol/*`: async frame IO, generic per-family `ProtocolHandler` with
  limiter + admission channel, outbound stream client.
- `ma2a-services` (new crate): target-side implementations (files, transfer, exec, shell, proxy) over
  a generic frame stream, plus client helpers used by the CLI.
- `ma2a-store`: schema 7 (`service_exposures`, `transfer_staging`).
- `ma2a-runtime`: authorization, inbound host wiring, stream plane, operations registry, API.
- `ma2a-app`: CLI; `web`: Services view and new member grants.

## Tasks (atomic commits)

1. Core capability bits + authorization with exposure (unit + property tests, golden vector intact).
2. Core frames + path model + message codecs (roundtrip, canonical rejection, bounds tests).
3. Store schema 7 + exposure/staging repository APIs (Store tests, migration test).
4. Runtime: `space_grant`, exposure commands, schema/TS update.
5. Net: service protocol handler/client; ALPN inventory; zero-Space isolation tests.
6. Services crate: Files host (unit tests on temp dirs incl. symlink/escape/overwrite cases).
7. Transfer host + client (integrity, resume, cancellation, staging cleanup, fault points).
8. Exec host (timeout, cancel, large output, backpressure, spawn failure).
9. Shell host (PTY resize/input/output/exit).
10. Proxy host (byte-exact echo server, half-close, refused, idle timeout, limits).
11. Runtime inbound wiring + audit + operations registry + restart sweep.
12. Stream plane IPC + outbound relay + replay classification (fault points before/after open).
13. CLI commands. 14. Web Services view. 15. Docs (protocol + CLI + security + limitations).
16. Multi-Endpoint E2E (acceptance gate) + regression + freeze.

## Deterministic fault points

Test-only hooks (compiled under `cfg(test)`/test features, never in release): stream relay drop
after `open` written; drop before `outcome` forwarded; upload crash after staging record before file;
after file before finalize; after finalize before record removal; store failure on replay recording.

## Acceptance gate (real multi-Endpoint E2E)

Files read/write/list; large transfer with BLAKE3 verification; transfer cancellation (+ staging
cleanup); one-shot Exec; interactive Shell resize/input/output; TCP Proxy byte-for-byte; unrelated
Endpoint denied; revoked Endpoint denied; one valid shared Space permits; two incomplete Spaces
(grant in S1, exposure in S2) denied; zero-Space Endpoint exposes no Phase 2 ALPN; limits bounded
(saturation returns `concurrency_exceeded`, permits released); restart leaves no active operation
and no staging residue. Phase 1 E2E remains green.

## As built (amendments recorded at implementation)

These refine the decisions above; where they differ, this section is authoritative.

- **D6 fingerprint.** BLAKE3 `derive_key("ma2a stream-plane open v1")` over the family code, the
  target Endpoint ID, and the exact canonical `open` payload. It is domain-separated from local
  API command fingerprints, which share the replay table.
- **D6 terminals.** A completed operation records its exact `outcome` frame, and a replay answers
  with that frame verbatim; there is no `stream_replayed` wrapper. An indeterminate operation is
  settled as a recorded failed `outcome(outcome_unknown)`, which its replay returns identically.
  A pending reservation with the same fingerprint answers `reject(unavailable)` while the
  operation runs in this process, and `outcome(outcome_unknown)` otherwise. A `reject` that
  arrives after the target's `accept` breaks the "nothing happened" promise, so the relay treats
  that stream as indeterminate instead of releasing its reservation.
- **D7 list.** Pages hold at most 128 entries, not 512.
- **D9 spawn failure.** A program that cannot start is a `reject(spawn_failed)` before any effect,
  not an outcome reason. Outcome reasons are `completed | timed_out | cancelled | shutdown`.
- **CLI shape.** Remote commands name the target with `--endpoint ID`, as `echo` does:
  `file --endpoint ID stat|ls|mkdir|rm|mv|cat|write|get|put`, `exec --endpoint ID [--stdin] -- …`,
  `shell --endpoint ID [-- …]`, `proxy connect --endpoint ID HOST PORT`, and
  `proxy forward --endpoint ID --listen ADDR HOST PORT`. `space grant` and
  `service expose|unexpose|list` cover consent, and `ops` shows operations. `exec` exits with the
  remote status: 124 on timeout, 130 on cancel, 128+N for signal N, and 255 on shutdown or an
  unknown status.
- **Client helpers.** The caller-side drivers live in `ma2a-services::client`, together with
  resumable download state (a partial file plus a durable sidecar naming the source) and local
  terminal raw mode. The Windows console-mode FFI is the audited unsafe boundary
  `client/terminal_windows.rs`.
- **Full duplex.** `FrameReader::next` is cancel-safe. The stream-plane relay and every caller
  driver run the two directions independently, so full-duplex traffic (for example,
  `exec --stdin -- cat` on a large input) can never deadlock.
- **Endpoint lifetime.** `RuntimeHandle` never stores Endpoint-holding values. The outbound
  `ServiceClient` is lent by the actor for each operation, because a handle may outlive its
  Runtime and would otherwise keep the old sockets bound.
- **Zero-Space callers.** An Endpoint sharing no Space with the target cannot even locate it,
  because address lookup is Space-scoped. Its attempts end as `unavailable` before any
  authorization decision is needed.
- **Consent ends with membership.** Every stored-chain write (import, enrollment, control sync,
  departure) withdraws, in the same transaction, the exposures of a Space whose chain no longer
  lists this Endpoint, and forgets file roots no remaining exposure binds. Rejoining needs fresh
  `service expose` consent.
- **Web Services view.** The console's Services route reads `GET /api/v1/services` and
  `GET /api/v1/operations`, polling every 3 s while it is open, and the Spaces screen shows each
  member's signed grants as neutral chips. Grants and consent stay CLI-only: no HTTP route changes
  them, so a browser session cannot make this Runtime serve anything.
- **Freeze audit fixes.** The end-of-phase audit found and this phase fixed:
  - Files containment raced by the caller's own `rename` (ancestor checked by `lstat`, then used
    by name). Resolution now holds directory handles: `openat(O_DIRECTORY | O_NOFOLLOW)` walks and
    `*at` final steps on Unix, and ancestors pinned without delete sharing on Windows.
  - Shell jobs in other process groups outliving their session. Cleanup now hangs up the leader's
    group, kills it, and on Linux kills every remaining member of the terminal session.
  - Revocation, grant removal, unexposure, or departure did not stop running operations. Every
    running inbound operation is now re-authorized after each durable change and stopped when it
    no longer passes.
  - Detached blocking Files work after a deadline (now bounded Runtime-wide), a Proxy caller's
    cancel after `end` being ignored, and a post-accept `reject` releasing a replay reservation.
