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
| `ma2a/relay/1` | relay advertisement | caller and target are current members, and the caller has relay-provider capability enabled by policy |

Unknown or future ALPNs and unsupported operations deny by construction.

## Control cursor

A control request may carry a typed Space cursor. The engine first derives shared Spaces from current membership. The cursor can then narrow processing to one of those already-derived Spaces. A cursor for an unrelated, missing, or revoked Space denies and never establishes authority.

## Permit and denial visibility

An allow produces an opaque in-process permit containing fixed-size internal audit context: `authorized_via`, manifest generation, and manifest hash. The permit has no serialization contract, exposes no audit accessors, and redacts its `Debug` representation.

Every failure returns the same `AuthorizationDenied` value and message. External behavior does not distinguish zero, one, or many shared Spaces; membership failure; revocation; capability failure; policy failure; cursor mismatch; or unsupported service. Space IDs, names, generations, hashes, and the selected allowing Space are never included in denial responses.

The runtime entry point accepts current verified views on every call and owns no authorization cache. Manifest advancement or learned revocation therefore affects the next decision as soon as the caller supplies the new committed views.

## Zero-Space protocol isolation

A zero-Space Runtime advertises and registers only `ma2a/enrollment/1`. Iroh ALPN negotiation rejects Echo, control, metadata, address-record exchange, relay advertisement, and unknown/future normal protocols before a bidirectional stream or service-body parser is available.

Enrollment remains a separate bootstrap authorization path based on the signed, short-lived, single-use invite contract. Establishing transport for enrollment does not authorize any normal operation.

The normal ALPN inventory is closed and maps every known normal protocol to a typed operation that requires the single runtime authorization entry point. No normal handler is registered in Todo 10; later handlers must use that mapping and entry point before reading service bodies.
