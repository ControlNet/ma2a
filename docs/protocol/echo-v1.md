# Echo Protocol v1

Echo v1 is a bounded encrypted request/response service for testing authenticated Endpoint reachability. It uses the exact Iroh ALPN `ma2a/echo/1` and the existing canonical request and response envelopes.

## Eligibility And Authorization

A Runtime advertises Echo only while it has at least one durable Space membership. Iroh authenticates the remote Endpoint ID during the cryptographic handshake. Before reading request-body bytes, Runtime checks the current persisted control-space state and admits the peer only when the two Endpoints share at least one currently valid Space that authorizes `echo_call`.

An unrelated peer receives the stable unauthorized transport status without body reading or canonical decoding. Peer-provided request fields never create authorization.

## Request

The initiator opens one bidirectional stream, writes one canonical Echo request, and finishes its send side. The request carries:

- service kind `Echo`
- request ID `[16]`
- exact target Endpoint ID `[32]`
- payload of at most 4,096 bytes

The complete canonical request remains within the shared wire bound. A version other than exactly 1.0 is a version mismatch. Noncanonical encodings, trailing bytes, target mismatch, and oversized payloads are invalid input.

## Response Frame

The responder writes one fixed header followed by an optional canonical response body:

`status:u8 || responder_endpoint_id[32] || duration_ms:u16 big-endian || body_len:u16 big-endian || body`

Status zero carries a canonical successful response containing the original request ID and exact accepted payload. Nonzero statuses carry an empty body. Stable status codes are version mismatch `1`, invalid input `2`, unauthorized `3`, timeout `4`, concurrency exceeded `5`, cancelled `6`, unavailable `7`, and internal `8`.

The client rejects a responder identity different from the requested target, whatever the status, a response request ID different from the initiating request, and a payload different from the one it sent. It also rejects as invalid input a nonzero status with a body, a duration above 10,000 milliseconds, and any byte after the frame.

## Bounds And Lifetime

- Payload: at most 4,096 bytes.
- Per authenticated peer: at most 16 concurrent streams.
- Across the Runtime: at most 128 concurrent streams.
- Aggregate client exchange deadline: 10 seconds.
- Aggregate server stream deadline: 10 seconds. Processing owns at most 9.9 seconds, reserving the final 100 milliseconds for bounded response-frame writing. A response frame the peer does not take within 100 milliseconds is abandoned and the stream reset.
- Refusals over the concurrency bounds: at most 128 concurrency-exceeded frames are written at once across the Runtime; a stream refused beyond that is reset with no reply.
- Duration metadata: saturated at 10,000 milliseconds.
- Semantic retries: none.

Concurrency permits use deterministic lifetime ownership and are released on success, rejection, timeout, cancellation, or task shutdown. Runtime metrics expose the current admitted-stream count so saturation and release can be observed without timing assumptions.

Runtime shutdown aborts owned Echo service work before closing the Iroh Endpoint. A cancellation status may be delivered only when the handler still has enough time and transport capacity to complete that frame; shutdown guarantees stream or connection closure, not delivery of status `6`.

## Audit And Metrics

Echo audit records contain only request ID, authenticated peer Endpoint ID, bounded result class, byte count, and bounded duration. Payload bytes, Space IDs, and authorization details are structurally absent. The in-memory audit retains at most 128 records. Outbound failures and inbound failures after canonical request decoding are recorded with their known request ID. Pre-body authorization failures, oversized frames, and malformed bodies without a decodable request ID intentionally cannot create an audit record.

Runtime exposes counters for bodies read after admission and canonical requests decoded. These counters allow tests to prove that unauthorized traffic is rejected before body processing.
