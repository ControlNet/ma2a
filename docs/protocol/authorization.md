# Endpoint-centric authorization

Normal remote requests identify an authenticated caller Endpoint and the local target Endpoint. A request does not select an authoritative Space. The receiver supplies its current verified `SpaceAuthorizationView` values and evaluates each independently.

## Decision rule

For each Space view, the authorization engine performs one complete evaluation:

1. The authenticated caller is a current member.
2. The local target is a current member.
3. Any typed resource applies only after those shared-membership checks.
4. The Space policy and member capabilities completely grant the requested operation.

Authorization succeeds when any one Space satisfies the entire rule. A denial or revocation in another Space cannot cancel that allow. Capabilities, memberships, or policy grants from separate Spaces are never combined.

The closed Phase 1 operation mapping is:

| ALPN | Operation | Complete per-Space grant |
| --- | --- | --- |
| `ma2a/echo/1` | Echo call | caller and target are current members with Echo enabled by policy and member capability |
| `ma2a/control/1` | control synchronization | caller and target are current members |
| `ma2a/metadata/1` | metadata read | caller and target are current members |
| `ma2a/address/1` | signed address-record exchange | caller and target are current members |
| `ma2a/relay/1` | relay advertisement | caller and target are current members, and private relay providers are enabled by Space policy |

The eight service ALPNs (`ma2a/files/1`, `ma2a/transfer/1`, `ma2a/exec/1`, `ma2a/shell/1`, `ma2a/proxy/1`, `ma2a/process/1`, `ma2a/agent/1`, `ma2a/service/1`) map to one service-stream operation that this engine always denies; `service::authorize_service` authorizes them under the rule in [Services v1](services-v1.md#authorization).

Unknown or future ALPNs and unsupported operations deny by construction.

## Control cursor

A control request may carry a typed Space cursor. The engine first derives shared Spaces from current membership. The cursor can then narrow processing to one of those already-derived Spaces. A cursor for an unrelated, missing, or revoked Space denies and never establishes authority.

## Permit and denial visibility

An allow produces an opaque in-process permit containing fixed-size internal audit context: `authorized_via`, manifest generation, and manifest hash. The permit has no serialization contract, exposes no audit accessors, and redacts its `Debug` representation.

Every failure returns the same `AuthorizationDenied` value and message. External behavior does not distinguish zero, one, or many shared Spaces; membership failure; revocation; capability failure; policy failure; cursor mismatch; or unsupported service. Space IDs, names, generations, hashes, and the selected allowing Space are never included in denial responses.

The runtime entry point accepts current verified views on every call and owns no authorization cache. Manifest advancement or learned revocation therefore affects the next decision as soon as the caller supplies the new committed views.

## Zero-Space protocol isolation

A zero-Space Runtime accepts only `ma2a/enrollment/1` on a new connection. Startup registers the Echo, control, and service handlers anyway, but sets the Endpoint's negotiated ALPN list to enrollment alone; joining the first Space widens that list, and leaving the last one narrows it again. Iroh ALPN negotiation rejects Echo, control, metadata, address-record exchange, relay advertisement, the service protocols, and unknown/future normal protocols before a bidirectional stream or service-body parser is available.

Enrollment remains a separate bootstrap authorization path based on the signed, short-lived, single-use invite contract. Establishing transport for enrollment does not authorize any normal operation.

The normal ALPN inventory is closed and maps every known normal protocol to a typed operation. Every registered normal handler checks authorization before it reads a service body.

## Inbound connection bound

Every ALPN is open to any peer that completes the handshake, so connections are counted before any handler runs. One peer may hold 64 inbound connections at once, whatever their ALPNs; one past that is closed at once, before its handler awaits a stream. A connection counts from accept until its handler returns, including the bounded wait for the peer to close after an enrollment or control answer. A peer that behaves needs at most 16 Echo calls, 8 enrollment exchanges and 2 control rounds at once, each on its own connection, plus one cached connection per service family. Each handler's own stream bounds still apply inside this one. There is no Runtime-wide connection bound (see [limitations](../limitations.md)).
