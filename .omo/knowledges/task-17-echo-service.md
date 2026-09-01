# Todo 17 Echo Service

- Reuse `RequestEnvelope` and `ResponseEnvelope` for canonical Echo bodies; do not introduce a second payload codec.
- Keep responder identity and bounded duration in the authenticated transport response metadata.
- Model `ServiceKind` as a closed enum so runtime dispatch remains exhaustive.
- Audit records retain only request ID, authenticated peer, result class, byte count, and bounded duration; payload and Space data are structurally absent.
- Register `ma2a/echo/1` only when the Runtime has durable Space membership, then authorize the authenticated peer against current persisted shared-Space state before reading body bytes.
- Use a RAII limiter for 16 concurrent streams per peer and 128 globally, with one ten-second aggregate deadline and no semantic retries.
- Real-Iroh verification should cover both an authorized shared-Space round trip and an unrelated zero-Space peer denied before body read or decode.
- Preserve typed pre-body failures: persisted authorization denial maps to `Unauthorized`, while Store/channel failure maps to `Unavailable` without exposing request bytes.
- The server-owned ten-second deadline must reserve bounded response-write time after processing expiry; the client independently maps its own aggregate expiry to `TimedOut`.
- Record outbound failures and inbound failures only after a canonical request ID is known. Authorization failures, oversized frames, and undecodable malformed bodies are intentionally unauditable because reading or inventing an ID would violate the pre-body boundary.
- Expose current admitted-stream count through the existing Echo metrics so real-Iroh saturation tests can synchronize on permit ownership without sleeps; RAII decrements the metric and limiter on every exit path.
- Keep the central actor dispatcher intact and extract membership observation into `actor/membership.rs`; this restored `actor.rs` to 247 policy LOC.

# Independent-verification correction

- Reserve response-write time inside a typed request/response server deadline. Echo processing now owns 9.9 seconds and the response frame is bounded by the original ten-second absolute deadline.
- A paused-time real-Iroh handler test must decode `EchoError::TimedOut` from status `4`; asserting only `Elapsed` does not prove transport behavior.
- Global saturation can be tested without 129 peers by pre-acquiring all production limiter permits, then observing `EchoError::ConcurrencyExceeded` from a real handler stream.
- Runtime shutdown guarantees owned service-task abortion followed by Endpoint closure. It does not guarantee transport-visible status `6`, so internal oneshot closure is not sufficient acceptance evidence.
