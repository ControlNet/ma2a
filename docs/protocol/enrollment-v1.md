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

The first valid redemption commits the complete canonical chain and the retry identity. The same Endpoint and RequestId may retrieve that committed chain after process restart. A changed RequestId or different Endpoint conflicts. Cancelled, expired, replay-conflict, invalid, and cross-Space requests return only a status byte, disclose no chain page, and do not advance Runtime revision or Space generation.

Redemption preserves the latest manifest's outstanding revocations when signing generation N+1. A candidate that is already a current member conflicts without consuming the invitation or creating a redundant generation.

## Page stream

A successful response is a sequence of length-prefixed `EnrollmentPage` frames after status zero. Each encoded page is at most 300,000 bytes and carries a zero-based index, total page count, final generation, Genesis only on page zero, and at most eight ordered canonical manifests. Page count is explicitly limited to 32 and is checked before allocation. There is no whole-chain response frame or 4 MiB aggregate response limit.

Core v1 permits at most 255 manifests, 64 active members, 64 disjoint revocations, and 64 UTF-8 bytes per member label. Those schema bounds keep every valid v1 chain below 4 MiB even though the generic signed-object ceiling is larger. The paging boundary is therefore the maximum valid 255-manifest chain, which streams as 32 independently bounded pages; implementations must not reintroduce an aggregate frame or buffer limit.

The candidate bounds every frame before allocation, decodes pages canonically, checks page indexes and counts, verifies Genesis and every authority signature, and applies every manifest in generation and previous-hash order. It then verifies the exact invited Space and local candidate membership before any durable write. Establishment occurs only after those checks and durable persistence of the resulting Core `SpaceChain`.

Connect, accept, read, write, mailbox admission, owner reply, and peer-close coordination are bounded. The complete exchange has a 30-second aggregate deadline, individual I/O and mailbox operations have ten-second deadlines, and no enrollment task or connection-close wait is unbounded or detached.

## Restart behavior

The owner persists the committed canonical Core chain and retry identity in the invitation transaction, independent of the network adapter. A retry reconstructs transport pages from that durable Core representation.

On candidate startup, the Store verifies persisted Space chains and loads the Spaces whose derived membership contains the local Endpoint. Runtime publishes those memberships before reporting ready, so normal Space protocols are eligible immediately after a successful restart.
