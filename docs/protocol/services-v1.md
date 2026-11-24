# Built-in Streaming Services v1

Phase 2 adds five Endpoint-targeted streaming services: Files, Transfer, Exec, Shell, and Proxy.
They share one frame format, one authorization rule, and one lifecycle contract. Echo v1 is
unchanged.

## Transport

Each family has one exact Iroh ALPN: `ma2a/files/1`, `ma2a/transfer/1`, `ma2a/exec/1`,
`ma2a/shell/1`, `ma2a/proxy/1`. A Runtime negotiates them only while it holds at least one Space.
A zero-Space Runtime advertises only `ma2a/enrollment/1`, so every service ALPN fails TLS
negotiation there. The ALPNs are advertised whatever the local exposure state is, so negotiation
never reveals what a Runtime serves.

One operation uses one QUIC bidirectional stream. Outbound connections are reused per exact target
and family.

## Authorization

The authenticated QUIC peer identity is the caller. A request is permitted only when **one single
Space** satisfies all of these at once:

1. both the caller and this Runtime are current members;
2. the caller's signed member grant in that Space holds the capability for the operation
   (`files_read` 4, `files_write` 8, `exec` 16, `shell` 32, `proxy` 64); and
3. this Runtime's local exposure for **that same Space** covers the resource: an exported root
   (read-only or read-write), `exec`, `shell`, or a proxy rule `HOST:PORT[-PORT]`.

Permissions from different Spaces are never combined, and nothing in a request names or reveals
the authorizing Space. A signed grant alone never makes a Runtime serve anything; the target's
operator opts in per Space with an exposure.

The decision is made twice, both times from durable store state at call time:

- **Coarse admission**, before any stream byte is read. Is there any Space where the caller may use
  this family at all?
- **Fine decision**, after the bounded `open` frame, for the exact decoded access: the root and read
  or write for Files and Transfer, or the exact host and port for Proxy.

A revocation, departure, or unexposure therefore applies to the very next stream. A denied caller
receives `reject(unauthorized)` and nothing else.

Authorization also holds for as long as an operation runs. Whenever durable state changes (checked
every second), each running inbound operation is authorized again for the exact access it opened
with, and must still resolve the same root directory. One that no longer passes is stopped: Exec,
Shell, and Proxy end with reason `shutdown`, and a Files or Transfer stream is reset. An
unfinished upload publishes nothing.

## Frames

Every frame is `type:u8 || length:u32 big-endian || payload`. The length is bounded by type
*before* any payload byte is read.

| Type | Name | Direction | Payload | Bound |
| --- | --- | --- | --- | --- |
| 1 | `open` | client → server, first | canonical CBOR request header | 1–16 KiB |
| 2 | `accept` | server → client | canonical CBOR accept header | 1–16 KiB |
| 3 | `reject` | server → client, terminal | status code `u8` | 1 byte |
| 4 | `data` | both | `channel:u8 || bytes` | 2 B – 64 KiB + 1 |
| 5 | `end` | both | `channel:u8` | 1 byte |
| 6 | `control` | both | canonical CBOR (resize, upload commit) | 1–16 KiB |
| 7 | `outcome` | server → client, terminal | canonical CBOR `{0: status, 1: result/null}` | 1–64 KiB |
| 8 | `cancel` | client → server | empty | 0 |

Channels are input `0`, output `1`, and error `2`. Proxy carries its byte stream on channel `0`
in both directions.

`reject` guarantees that no side effect happened. An `outcome` means the operation started, and
it may report a failure. A stream reset or connection loss counts as `cancel` on the target. To
the caller it is indeterminate once `open` has been sent.

The `open` header is the map `{0: [major 1, minor 0], 1: request_id[16], 2: target_endpoint_id[32],
3: service code, 4: body}`. Any version other than exactly `[1, 0]` is rejected with
`version_mismatch`; how services themselves are versioned is in `docs/protocol/service-v1.md`. A target
that differs from the receiving Runtime, a family that differs from the negotiated ALPN, trailing
bytes, and noncanonical encodings are all `invalid_input`.

## Status codes

`version_mismatch` 1, `invalid_input` 2, `unauthorized` 3, `timed_out` 4, `concurrency_exceeded`
5, `cancelled` 6, `unavailable` 7, `internal` 8, `conflict` 9, `outcome_unknown` 10, `not_found`
16, `already_exists` 17, `not_a_directory` 18, `is_a_directory` 19, `not_empty` 20,
`symlink_refused` 21, `invalid_path` 22, `permission_denied` 23, `read_only_root` 24, `too_large`
25, `io` 26, `integrity_mismatch` 32, `content_changed` 33, `precondition_failed` 34,
`spawn_failed` 48, `connection_refused` 64, `unreachable` 65, `dns_failed` 66.

Operating-system error text, paths, and raw OS codes never leave the target.

## Files v1

**Paths.** A path is `ROOT:/a/b`: a root name (`[a-z0-9][a-z0-9._-]*`, at most 64 bytes) plus at
most 64 components, each 1–255 UTF-8 bytes, at most 4,096 bytes in total. The following components
are invalid:

- `.`, `..`, and empty components;
- components containing NUL, `/`, `\`, `:`, or control characters;
- on Windows hosts, reserved device names and names with a trailing dot or space.
- names starting with `.ma2a-upload-` or `.ma2a-write-`, in any ASCII letter case: the Runtime's
  own staging files, which a caller can neither read nor replace while an upload or write is being
  verified and published.
- names starting with `.ma2a-downloads`, in any ASCII letter case: the private directory a local
  download keeps its partial file in (see "Resumable downloads"). A root whose own path passes
  through such a directory serves nothing and answers `invalid_path`.

Nothing is resolved lexically. A root name is bound Runtime-wide to one canonical absolute
directory. Each Space exposes it read-only or read-write.

**Containment.** Every ancestor below the root is checked with `lstat`, and symbolic links are
never followed:

- a symlink ancestor fails with `symlink_refused`;
- a final symlink is reported as `symlink` (its target is never disclosed), can be renamed or
  removed as a link, and is refused for reading and writing;
- devices, sockets, and FIFOs are never opened (`invalid_path`).

Resolution never re-walks a checked path by name. Each component is opened from the handle of its
parent without following links (`openat` with `O_DIRECTORY | O_NOFOLLOW` on Unix), and the final
step (`statat`, `openat`, `mkdirat`, `unlinkat`, `renameat`, `linkat`) runs relative to the held
parent. Swapping an ancestor for a symbolic link while an operation runs, including through the
caller's own concurrent `rename`, therefore cannot redirect the operation outside its root. On
Windows every ancestor from the root down stays open without delete sharing for the whole
operation, so no ancestor can be renamed or replaced meanwhile, and reparse points are refused.

| Op | Code | Access | Semantics |
| --- | --- | --- | --- |
| stat | 0 | read | kind, size, modified ms, read-only flag |
| list | 1 | read | byte-order sorted page of at most 128 entries after an opaque cursor; memory bounded by the page; names a path cannot address (not UTF-8, not a valid path component, such as one with a control character, `\`, or `:`, or longer than 255 UTF-8 bytes, which a Windows volume allows), names the Runtime reserves (see above), and children whose path would pass 64 components or the path byte bound are left out |
| mkdir | 2 | write | `parents`: missing ancestors are created and an existing directory succeeds; otherwise `already_exists` |
| read | 3 | read | at most 64 KiB from an offset: `accept`, one `data` (output), then `outcome {length, eof}` |
| write | 4 | write | at most 64 KiB: `accept`, then the client sends `data` (input) and `end`; `create_new` publishes by hard link (falling back to the platform no-clobber rename) and `replace` by atomic rename, both after fsync of a temporary sibling |
| rename | 5 | write | same root only; no-replace uses `renameat2(RENAME_NOREPLACE)` (Linux), `renamex_np(RENAME_EXCL)` (macOS), or `MoveFileExW` without the replace flag (Windows) |
| remove | 6 | write | file, link, or empty directory; `recursive` removes a tree without following links |

Content-free operations reply with `reject` (nothing happened) or `outcome`. A recursive
`remove`, or a `mkdir` with parents, that fails after changing something reports a failed
`outcome` rather than a `reject`. A recursive `remove` keeps every download private directory
(`.ma2a-downloads`, in any letter case) it meets, with everything in it, so a paused or running
download keeps its progress. It enters at most 64 levels of directories below the one removed,
as many as a path can name, and keeps anything deeper the same way. It removes everything else,
leaves the directories above a kept one, and ends with a failed `outcome` of `not_empty`. It removes one entry at a time on every platform
and never enters a link, junction, or other reparse point; it removes the link itself. Every Files operation has a 15-second deadline, and a stream that
exceeds it is reset.

## Transfer v1

The resource is `File(ROOT:/path)`. A download needs `files_read`. An upload needs `files_write` on
a read-write root.

**Download** `Get(resource, offset, expected_identity?)`:

1. The server sends `accept {size, modified_ns}`.
2. It sends `data` (output) chunks.
3. It sends `outcome {bytes, size, blake3}`.

The server hashes the *full* content, including a skipped resume prefix. A resumed download
(offset > 0) must name the identity it continues. A mismatch is rejected with
`precondition_failed`, and the client restarts. If the source grows, shrinks, or changes its
modification time while streaming, the outcome is `content_changed`. The client verifies BLAKE3
over its final file. It keeps the partial file and a sidecar with the source identity, so a resume
survives any daemon restart.

**Upload** `Put(resource, size, mode)`:

1. The server records the staging path `.ma2a-upload-<request_id><nonce>.part` in SQLite
   *before* creating the file exclusively. The nonce is 16 random bytes, so each attempt has its
   own name and no entry already in the directory carries it; when the file cannot be created,
   the record is dropped at once.
2. It sends `accept`.
3. The client sends exactly `size` bytes of `data` (input), then `end`, then
   `control {commit: blake3}`.
4. The server checks the size and hash, fsyncs, and publishes atomically with `create_new` or
   `replace`.

Every failure, cancellation, and stall removes the staging file. Startup removes every recorded
staging file before the Runtime serves any stream. Upload resume is not supported in v1. While an
upload runs, its staging file is an ordinary entry of the destination directory on the host, but no
remote caller can list, read, or replace it: Files and Transfer leave `.ma2a-upload-*` names out of
`list` and answer `invalid_path` for any path that names one.

Progress stalls longer than 30 seconds fail with `timed_out`. Memory use is one 64 KiB frame, and
QUIC flow control provides backpressure.

## Exec v1

`open {program, args[], cwd?, env[], stdin: bool, timeout_ms}`:

- **Arguments are typed argv**; nothing is ever interpreted by a shell.
- **Program resolution.** `program` is resolved through the target Runtime's `PATH`, or is an
  absolute path.
- **Working directory.** `cwd` must be absolute; if omitted, the target's home directory is used.
- **Environment.** At most 256 arguments and 64 environment additions
  (`[A-Za-z_][A-Za-z0-9_]*`). Each text value is at most 4,096 bytes, without NUL.
- **Timeout.** 1 ms – 24 h.

A program that cannot start is rejected with `spawn_failed` before any effect. Otherwise:

1. The server sends `accept`.
2. stdin flows as `data` (input) plus `end`.
3. stdout flows as `data` (output) and stderr as `data` (error).
4. The server sends `outcome {termination: exited(code) | signaled(sig) | unknown, reason, duration_ms,
   stdout_bytes, stderr_bytes}`.

`reason` is `completed`, `timed_out`, `cancelled` (caller cancel or stream loss), or `shutdown`.
On Unix the child runs in its own process group, and the whole group is killed at the end of every
run: after exit (with a 250 ms output drain), on timeout, cancellation, loss, and Runtime shutdown.
The group ID is captured at spawn. On Windows only the direct child is killed; this is a documented
limitation.

## Shell v1

`open {program?, args[], cwd?, env[], term, cols, rows, max_lifetime_ms}` runs a program on a PTY
(Unix) or ConPTY (Windows) using `portable-pty`. The default program is `$SHELL`, falling back to
`/bin/sh`, or `%COMSPEC%` on Windows. `TERM` is set from `term`.

- Input is `data` (input); `end` stops input while the session continues.
- Resize is `control` with `{0: cols, 1: rows}`.
- Terminal output is `data` (output).

The session ends when its program exits, when its lifetime (at most 24 h) expires, on caller
cancel or loss, and on Runtime shutdown. At the end every process the session started is ended,
including jobs a job-control shell moved into other process groups: the leader's group receives
SIGHUP (interactive shells forward it to their jobs), then SIGKILL, and on Linux every remaining
process of the terminal session is found in `/proc` and killed. A program that leaves the session
itself (`setsid`) is not reached, and on hosts without `/proc` a job that ignores SIGHUP in another
group may survive. A program ended by a signal reports termination `unknown`, because the PTY layer
exposes only signal names. Sessions never survive a restart.

## Proxy v1

`open {host, port, idle_timeout_ms}` connects the target to one TCP destination:

- **Rule check.** The rule is checked against the host *as requested* (a lowercase DNS name or a
  canonical IP literal), not as resolved, so DNS answers can never widen an operator's rule.
- **Connect.** Resolution plus connection has a 10-second budget. Failures are rejected with
  `connection_refused`, `unreachable`, `dns_failed`, or `timed_out`.
- **Relay.** After `accept`, channel 0 carries bytes in both directions, byte for byte. `end` maps
  to TCP `shutdown(Write)`, and destination EOF becomes `end`.
- **End of stream.** The stream ends when both directions have ended, when the idle timeout (at
  most 1 h) or the 24 h lifetime expires, on cancel, or on Runtime shutdown. After the caller's
  `end`, the target keeps reading its stream, so a later `cancel` or reset still ends the relay at
  once even while the destination stays silent.
- **Outcome.** `{reason, bytes_to_target, bytes_from_target, duration_ms}`. A failure of the
  destination socket after connect is an `unreachable` outcome.

## Limits

| Family | Per peer | Runtime-wide | Deadline |
| --- | --- | --- | --- |
| Files | 16 | 64 | 15 s per operation |
| Transfer | 4 | 16 | 30 s without progress |
| Exec | 8 | 32 | request timeout ≤ 24 h |
| Shell | 4 | 16 | lifetime ≤ 24 h |
| Proxy | 64 | 256 | idle ≤ 1 h, lifetime 24 h |

A stream over a bound is rejected with `concurrency_exceeded` before any byte is read. The
`open` frame must arrive within 15 seconds. Permits are RAII and released on every exit path.
A peer the Runtime refuses outright, such as a non-member, gives its permit back before the
`reject` frame is written, so refusals cannot fill a bound.
A host waits at most 5 seconds to write the frame that opens or ends an operation (`accept`,
`reject`, or `outcome`) and to close the stream, and a Process supervisor must answer within
10 seconds, so a caller that stops reading, or a supervisor that stops answering, cannot hold a
host, its permits, or a started child past that, whether or not the grant is revoked meanwhile.
At most 16 blocking Files jobs run at once Runtime-wide; a job reset at its deadline keeps its slot
until it actually finishes, so abandoned work (a huge recursive removal or listing) cannot pile up.
A recursive removal whose stream ends first, because it was cancelled, its grant revoked, or its
deadline passed, removes nothing more: it checks before each entry and stops, so part of the tree
may already be gone, but no later entry is touched. A `stat`, `list`, `mkdir`, `rename`, or
`remove` notices the caller's `cancel` or reset while it runs and answers a failed `cancelled`
outcome, since what it did so far may stand.

## Audit

Each Runtime keeps an in-memory audit of the most recent 256 operations: request ID, peer,
family, operation name, direction, result class, byte counts, and duration. Paths, argv,
environment, file content, terminal bytes, and proxy bytes are structurally absent. The audit and
the active-operation list are not persisted, so after a restart no operation appears active.

## Caller side

A local client never holds Endpoint identity. It upgrades one local API connection with
`stream_open` (see `docs/protocol/local-api-v1.md`), sends exactly one `open`, and then speaks the
family contract above over that connection. The Runtime dials the target as itself and relays
frames verbatim. The two directions are relayed independently, so full-duplex operations never
stall.

A caller keeps its write side open until it receives the terminal frame. Closing it earlier
cancels the operation, and a stream that ends without a terminal frame is reported as
`outcome_unknown`.

The `ma2a` CLI drives every family this way:

| Command | Family | Notes |
| --- | --- | --- |
| `file --endpoint ID stat\|ls\|mkdir\|rm\|mv\|cat\|write` | Files | `cat` reads in 64 KiB steps; `write` takes at most 64 KiB from stdin |
| `file --endpoint ID get PATH LOCAL [--resume] [--force]` | Transfer | BLAKE3 over the whole content is verified before `LOCAL` is published |
| `file --endpoint ID put LOCAL PATH [--replace]` | Transfer | published only after the target verified size and hash |
| `exec --endpoint ID [--stdin] [--cwd] [--env] [--timeout] -- PROGRAM ARGS…` | Exec | exits with the remote status: 124 on timeout, 130 on cancel, 128+N for signal N, 255 on shutdown |
| `shell --endpoint ID [--lifetime] [-- PROGRAM ARGS…]` | Shell | raw local terminal; its size and resizes are forwarded |
| `proxy connect --endpoint ID HOST PORT` | Proxy | relays stdin and stdout, e.g. as an SSH `ProxyCommand` |
| `proxy forward --endpoint ID --listen ADDR HOST PORT` | Proxy | one stream per accepted local connection, at most 64 at once |

**Resumable downloads.** A download keeps its helper files in `.ma2a-downloads/`, a directory
beside `LOCAL` that only the current account may enter. On Unix it is made with mode `0700`, and
an existing one is used only when the current account owns it and its mode grants nothing to
group or others. On Windows it is made with a protected ACL that grants only its owner and SYSTEM
and is inherited by every helper file, and an existing one is used only when it is owned by the
current user, SYSTEM, or Administrators and its protected ACL allows no other account in.
Otherwise the download fails with `AlreadyExists` and leaves the directory as it was. On Windows
this refuses a `.ma2a-downloads` made by earlier builds, which inherited `LOCAL`'s directory's
ACL; remove it, once no download uses it, to let a download make a private one. A `LOCAL` named `.ma2a-downloads`, in any letter
case, is refused with `InvalidInput` before anything is created. A download that makes the
directory marks it with a new `created` file holding exactly `ma2a-download-directory v1` and a
newline, and the last download to end in it removes it. One without that mark, such as one the
operator made, is kept when downloads end, along with any `created` file of other content and any
`locks` directory the operator made; a download removes only a `locks` directory it made itself
or one in a directory a download made.
The partial file is `HASH.ma2a-part`, where `HASH` is 32 hex characters of BLAKE3 over `LOCAL`'s
exact file name, so no helper file shares a name with an ordinary file or another download's
destination, and two destinations a case-sensitive file system keeps apart (`report`, `REPORT`)
keep their own progress. The lock file is `locks/NAME`, carrying `LOCAL`'s own file name, so the
file system itself decides which spellings name one destination: whatever the volume's case or
normalization rules, one destination has one writer, and two destinations the volume keeps apart
never wait for each other.
Before the first content byte, the download durably records the source (target Endpoint and
remote path, in its canonical spelling) and the content identity in `HASH.ma2a-part.meta`.
`--resume` continues only a partial file whose sidecar names the same source, and it re-hashes
the local prefix so verification still covers the whole content. A resumed download answered
with another identity fails and keeps its sidecar as it was. If the target answers `precondition_failed` (the
source changed), the download restarts once from zero. A partial file whose content changed or
failed verification is removed. While a download runs, it holds an OS lock on its lock file,
so a second download to the same `LOCAL` fails at once instead of writing alongside it; the
lock file is removed when the download ends.
A download opens `LOCAL`'s directory, the private directory, and its lock directory once, when it
starts, and reaches every helper file through them (on Unix by `*at` calls; on Windows each is
held open without delete sharing, so neither it nor a directory above it can be renamed
meanwhile, and it is reached by the path its handle resolves to, with every junction and link
resolved). Publishing first checks that `LOCAL`'s directory path still names the directory the
download started in, and that the partial file is still the one it hashed (by device and inode
on Unix, by volume serial number and file index on Windows); afterwards, that `LOCAL` names that
file. On Windows the partial file, once open, shares only reading, so no other handle can write
it while the download runs. If the directory was moved or
replaced, nothing is published, the download fails, and its progress stays in the private
directory where the original directory now is, for `--resume` there. The verified partial file is
then renamed into place, so the published file is the one whose content was verified.
Even inside that directory, a download takes over only helper files it can show it wrote: a
sidecar, or its staging file `HASH.ma2a-part.meta.tmp`, that starts with the MA2A header, a
partial file next to such a sidecar or still empty, and an empty lock file. Any other file or
link that carries one of these names, or a `.ma2a-downloads` that is not a directory, fails the
download with `AlreadyExists` before anything is written, and keeps its content. Helper files
that earlier builds kept directly beside `LOCAL` (`LOCAL.ma2a-part` and its suffixes) are never
read, resumed, or removed. Neither are the lock files of builds before this layout
(`HASH.ma2a-lock`, `HASH.ma2a-part.lock`), nor a partial file that a build before `b1c8d13` named
by the lowercased name of a `LOCAL` with uppercase letters: such a download starts over, and the
old files can be removed by hand once no older client runs. Stop older clients before using a
newer one on the same destination; the two take different locks.
Without `--force`, `LOCAL` is published by a rename that never replaces an existing file
(`renameat2` with `RENAME_NOREPLACE` on Linux, `RENAME_EXCL` on macOS, `MoveFileExW` without
replacement on Windows, and a hard link elsewhere or where the file system lacks such a rename).
On a file system with neither, publishing fails rather than risk replacing a file that appeared
meanwhile.

**Consent and grants.** The Space owner sets a member's grants with
`space grant SPACE_REF ENDPOINT_ID [--files-read] [--files-write] [--exec] [--shell] [--proxy]
[--process] [--agent] [--service]`. One grant replaces all eight bits at once: every flag left
out is cleared, including the Process, Agent, and Service grants of later phases, and work that
relied on a cleared grant, such as a Process or an Agent task, is withdrawn as its protocol
describes. To change one bit, repeat every flag the member should
keep, for example `space grant SPACE_REF ENDPOINT_ID --files-read --files-write --process` to add
write access for a member that already reads files and manages Processes. Each serving Endpoint consents for itself with
`service expose|unexpose files|exec|shell|proxy SPACE_REF …`, and `service list` shows what it
serves. `ops` shows active and recent operations. Consent lasts only while the serving
Endpoint belongs to the Space: once a stored chain no longer lists it (it left or was removed),
that Space's exposures are withdrawn atomically with the chain update, and rejoining needs fresh
consent.
