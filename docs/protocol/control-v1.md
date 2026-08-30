# Control Synchronization Protocol v1

Control v1 reconciles signed Space manifests, target-specific address records, and Private Relay advertisements between existing members. It uses the exact Iroh ALPN `ma2a/control/1` and does not require a pre-existing connection.

## Eligibility and authorization

A zero-Space Runtime advertises enrollment only. The control handler is registered on the long-lived router, but `ma2a/control/1` becomes eligible for new incoming connections only after durable local membership exists. Enrollment completion enables control without replacing the Endpoint or changing its Endpoint ID.

Iroh authenticates the remote Endpoint ID before Runtime parses the control request. Runtime invokes the centralized endpoint-oriented remote authorization seam before decoding peer-controlled bytes, then independently derives the currently valid Spaces shared by the local and remote Endpoints. No authorized shared Space returns an authorization rejection. Request cursors and push pages are accepted only when every contained Space ID belongs to that independently derived set; a peer-provided Space ID only narrows authority and never creates authorization.

## Transport framing

The initiator opens one bidirectional stream, writes one canonical request of at most 1,048,576 bytes, and finishes its send side. The responder reads to EOF with the same cap.

The response frame is `status:u8 || body_len:u32 big-endian || body`. Status zero carries one canonical `ControlResponseV1`; nonzero status carries an empty body. Stable rejection statuses are unauthorized `1`, invalid `2`, and unavailable `255`. Connect, stream admission, reads, writes, mailbox delivery, and replies have ten-second I/O deadlines.

## Canonical request

A request begins with `MCQ1`, followed by a big-endian `u16` cursor count. At most 64 Space cursors are allowed and they are canonically sorted by Space ID without duplicates.

Each `ControlCursorV1` contains:

- `space_id[32]`
- latest accepted manifest generation as big-endian `u64`
- latest accepted manifest chain hash `[32]`
- up to 256 sorted unique address entries `(endpoint_id[32], sequence:u64)`
- up to 256 sorted unique relay entries `(provider_endpoint_id[32], sequence:u64)`

The cursor section is followed by up to 64 opportunistic push pages. Decoding rejects over-limit counts, unknown artifact kinds, invalid booleans, trailing bytes, noncanonical ordering, duplicates, or any re-encoding mismatch.

## Canonical response and pages

A response begins with `MCR1` and contains up to 64 Space-local pages. Request and response pages share this encoding:

`space_id[32] || more:u8 || artifact_count:u16 || artifacts`

Each page contains at most 256 artifacts. An artifact is `kind:u8 || signed_len:u32 || signed_bytes`, where kind `0` is a signed manifest, `1` is a signed Space address record, and `2` is a signed Private Relay advertisement. The complete encoded request or response remains within 1 MiB.

The sender forwards original canonical signed bytes unchanged. It does not re-sign, normalize, or attribute trust to the forwarding peer. `more = 1` means another bounded round may retrieve additional artifacts.

## Reconciliation

For each authorized shared Space, the responder verifies the receiver's manifest generation and hash against its local contiguous chain. It returns the missing contiguous manifest suffix, then current unexpired address and relay artifacts whose signed sequence exceeds the corresponding receiver cursor.

Opportunistic push pages carry the sender's bounded contiguous manifest chain plus current unexpired address and relay artifacts. A receiver accepts already-known manifests only when their canonical bytes exactly match its accepted chain, applies newer manifests sequentially through `SpaceChain`, and rejects gaps or forks.

Every received address record passes the existing exact-target signer, Space membership, clock, signature, sequence, rollback, and fork validator. Every relay advertisement passes the existing provider signer-binding, Space authorization, clock, signature, sequence, rollback, and fork validator. A malformed, expired, stale, forked, signer-mismatched, or cross-Space artifact fails closed. Complete pages are decoded, validated, and staged before manifests, address records, and relay advertisements are committed in one SQLite transaction with one Runtime revision increment; a later invalid artifact leaves every earlier artifact unapplied.

After persistence, Runtime atomically refreshes the private address lookup authorization snapshots and validated records. Learned revocation therefore changes control eligibility and target resolution immediately for that Space without changing independent Spaces.

## Scheduling and bounds

Each Space selects at most four fresh, known, unrevoked peer Endpoints per round and rotates the selection offset across rounds. Peers are deduplicated across Spaces. Runtime resolves them only through the private Space address lookup and allows at most eight concurrent active dials.

Each peer receives at most three attempts, and only transient transport or unavailable-service failures are retried. The delay before each retry is cryptographically sampled full jitter in the inclusive range from zero through the capped exponential delay; permanent authorization, framing, validation, and peer-rejection failures return immediately. The complete scheduled round, including all peer batches and response application, has one 30-second aggregate deadline. A round with prepared peers but no successful exchange reports failure; an empty round completes without network work.

Runtime owns and joins control tasks. The seven trigger classes are startup, accepted manifest advancement, accepted SpaceAddressRecord advancement, accepted PrivateRelayAdvertisement advancement, post-enrollment completion, explicit synchronization, and a deterministic per-Endpoint periodic interval in the bounded range 60 through 89 seconds. Concurrent triggers merge into one typed pending scope. A peer-targeted explicit request retains every requested peer that is currently eligible instead of replacing it with the rotating global fanout window. Explicit waiters are indexed by the round they requested, so completion of an older in-flight round cannot complete a newer peer-targeted or state-wide request; each waiter reports the success or failure of its own active or pending round.
