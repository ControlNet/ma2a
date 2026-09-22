# Enrollment Protocol v1

Enrollment v1 adds one Endpoint to one signed Space without transferring authority keys.

## Invitation ticket

An invitation is an authority-signed `ma2ainvite` ticket containing version 1, a 16-byte invitation ID, Space ID, owner Endpoint ID and address, issuance and expiry times in Unix milliseconds, and a 32-byte random secret. The ticket signature covers every field with the `ma2a-enrollment-invite-signature-v1` domain.

Callers request only a positive lifetime of at most 300,000 milliseconds. The owner Runtime reads its injected `RuntimeClock`, sets the signed creation time, and computes expiry with checked addition. Signing, decoding, and signature verification reject any zero, reversed, overflowing, or longer interval.

The plaintext secret exists only in the ticket and is zeroized when ticket or entropy values are dropped. SQLite stores `BLAKE3("ma2a-enrollment-invite-digest-v1" || secret)` and never stores or logs the plaintext. The invitation row also stores the signed creator Endpoint ID, owner-issued creation time, expiry, and bounded canonical owner bootstrap address so redemption can compare the complete persisted metadata before mutation.

## Authoritative time

The candidate request contains the signed ticket, RequestId, and display name. It contains no redemption timestamp. The owner Runtime obtains redemption time from its injected `RuntimeClock`; production uses `SystemClock` and deterministic tests inject controlled clocks. Only this owner time decides expiry and the issued time of the membership manifest.

## Single use and retry

Pending invitations transition atomically with the new signed Space generation. The transaction binds the verified ticket invitation ID, ticket Space ID, domain-separated secret digest, TLS-authenticated candidate Endpoint ID, and RequestId.

The first valid redemption commits the complete canonical chain and the retry identity. The same Endpoint and RequestId may retrieve that committed chain after process restart. A changed RequestId or different Endpoint conflicts. Cancelled, expired, not-yet-valid, replay-conflict, invalid, and cross-Space requests return only a nonzero status with a zero page count, disclose no chain page, and do not advance Runtime revision or Space generation.

Redemption preserves the latest manifest's outstanding revocations when signing generation N+1, except for the candidate's own. A previously removed Endpoint therefore rejoins only through a fresh owner-signed invitation, and generation N+1 carries it as a member instead of a stale revocation; earlier generations keep the revocation they recorded. A candidate that is already a current member conflicts without consuming the invitation or creating a redundant generation.

## Departure

A member leaves a Space over the same reserved enrollment ALPN. A departure request is exactly `"MLV1" || space_id:32 || request_id:16`; its magic prefix distinguishes it from an enrollment attempt, and any other framing is rejected. The transport authenticates the requesting Endpoint, so a request can only ever remove its sender.

The authority accepts the request only for a Space it holds the authority key for and only when the requester is a current signed member that is not the Space's own initial member. It signs the next generation removing that Endpoint from `members` and adding it to `revocations`, exactly as an owner-initiated revocation does. It then replies with status zero and the advanced chain split into the same bounded `EnrollmentPage` stream enrollment uses, so a long-lived Space's history stays inside the transport's frame bounds instead of being shipped as one whole-chain frame. A request from an Endpoint that was already removed and revoked returns that same chain, so a repeated departure converges instead of failing. Anything else returns a nonzero status and no frame.

The leaving Runtime reconstructs the chain with the same page validator enrollment uses, so genesis, every authority signature, page ordering and contiguous generations are re-verified before anything else. It then persists nothing until it has additionally verified that the chain has the same genesis and Space ID, does not regress, excludes the requester from `members`, and revokes it; persistence keeps its existing rollback and fork protections. An unreachable or refusing authority leaves local membership untouched, so a departure is never reported as succeeding without an authority-signed removal. Phase 1 has no authority transfer, so a Space's own owner cannot leave it.

## Response framing

The response wire format is exactly `status:u8 || frame_count:u16 big-endian || frame_count * (frame_len:u32 big-endian || frame_bytes)`. Status zero requires `frame_count` in `1..=32`; every nonzero status requires `frame_count` zero. The candidate validates this count before reserving frame storage or allocating any frame buffer, reads exactly the declared frames, bounds each nonzero frame length before allocation, and rejects bytes after the declared final frame.

Each success frame is a canonical enrollment bootstrap frame encoded as `"MEB1" || enrollment_page_len:u32 big-endian || enrollment_page || owner_address_len:u32 big-endian || owner_address`. The first frame requires the exact canonical current `SignedSpaceAddressRecordV1` for the owner Endpoint; every later frame requires `owner_address_len` zero. The embedded `EnrollmentPage` remains at most 300,000 bytes and carries a zero-based index, total page count, final generation, Genesis only on page zero, and at most eight ordered canonical manifests. The signed address record remains at most 16,384 bytes, making the complete frame bound 316,396 bytes. There is no whole-chain response frame or 4 MiB aggregate response limit. The owner validates status/count semantics and every frame length before writing the response header or body.

Core v1 permits at most 255 manifests, 64 active members, 64 disjoint revocations, and 64 UTF-8 bytes per member label. Those schema bounds keep every valid v1 chain below 4 MiB even though the generic signed-object ceiling is larger. The paging boundary is therefore the maximum valid 255-manifest chain, which streams as 32 independently bounded pages; implementations must not reintroduce an aggregate frame or buffer limit.

The candidate bounds every frame before allocation, decodes the bootstrap and embedded pages canonically, checks page indexes and counts, verifies Genesis and every authority signature, and applies every manifest in generation and previous-hash order. It then verifies the exact invited Space, local candidate membership, owner Endpoint identity from the invitation, owner membership authorization, owner address signature, issue and expiry times, and exact canonical owner address bytes before any durable write. The candidate commits the resulting Core `SpaceChain` and owner address high-water under one SQLite transaction and one Runtime revision. After that commit it publishes its own Space address and queues an immediate control round targeted to the owner Endpoint.

Connect, accept, read, write, mailbox admission, owner reply, and peer-close coordination are bounded. The complete exchange has a 30-second aggregate deadline, individual I/O and mailbox operations have ten-second deadlines, and no enrollment task or connection-close wait is unbounded or detached.

## Restart behavior

The owner persists the committed canonical Core chain and retry identity in the invitation transaction, independent of the network adapter. A success or exact retry reconstructs bootstrap frames from that durable Core chain and the owner's current persisted signed Space address record; denial and conflict status semantics remain unchanged.

On candidate startup, the Store verifies persisted Space chains and loads the Spaces whose derived membership contains the local Endpoint. Runtime publishes those memberships before reporting ready, so normal Space protocols are eligible immediately after a successful restart.
