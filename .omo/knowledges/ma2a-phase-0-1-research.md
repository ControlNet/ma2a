# MA2A Phase 0/1 research ledger

## Repository baseline

- Greenfield implementation repository: only tracked file is `README.md` (`# ma2a`).
- Only commit: `4f252185a6e7c7013f7b807dd59dea54ae32060e` (`Initial commit`).
- `master` and `origin/master` are aligned.
- No Cargo manifests, source, tests, CI, toolchain pin, project AGENTS.md, or existing conventions.
- `.omo/` contains untracked planning/session artifacts and must be preserved.

## Locked MA2A ontology and authorization semantics

- Universe is conceptual only: no `UniverseId`, global registry, join operation, authority, or universally replicated database.
- Space is the privacy, visibility, membership, and policy boundary. Nothing crosses a Space boundary implicitly.
- A Runtime owns exactly one persistent Iroh Endpoint that independently belongs to `0..N` Spaces.
- Persist Endpoint identity separately from `SpaceMembership(space_id, endpoint_id, ...)`; never put `space_id` inside Endpoint identity.
- A zero-Space Endpoint is valid but denies all normal remote discovery, metadata, and Services. Only a valid narrowly scoped invite/enrollment flow is available.
- Normal operations address a target Endpoint, not a Space. The receiver computes shared Spaces internally.
- Each shared Space policy is evaluated independently. Authorization succeeds if one Space authorizes the complete operation; denies in other Spaces do not cancel that complete allow.
- Never combine partial permissions from different Spaces. `authorized_via` is internal audit/debug context, not a normal caller-supplied parameter.

## Iroh Core 1.1 contracts

Verified tag/commit: `iroh v1.1.0`, `fddf1a4ce29f92c6651eccff68fb366007b9be7d`.

- Persist `SecretKey` and supply it to the endpoint builder to preserve `EndpointId`.
- One Endpoint can route multiple ALPN handlers through `RouterBuilder`.
- `Connection::paths()` is a snapshot; `paths_stream()`/`path_events()` are live state mechanisms.
- `Connection::stable_id()` is only stable for one connection lifetime.
- Runtime `Endpoint::insert_relay`/`remove_relay` mutate relay configuration; they do not prove existing connection migration.
- Avoid feature-gated unstable net-report/custom-transport APIs in the core contract.

Primary sources:

- https://github.com/n0-computer/iroh/tree/fddf1a4ce29f92c6651eccff68fb366007b9be7d
- https://docs.rs/iroh/1.1.0/
- https://docs.iroh.computer/concepts/address-lookup
- https://docs.iroh.computer/about/release-policy

## Discovery and privacy

- N0 default DNS/Pkarr lookup publishes endpoint addressing metadata.
- Strict private Spaces should use `presets::Minimal` or a custom preset/address lookup.
- Phase 1 must not use `presets::N0` unchanged. Compose private MA2A `AddressLookup`, explicit relay configuration, and optional N0 public relays only as transport fallback.
- N0 public relay use is distinct from N0 DNS/Pkarr address publication; public publication remains disabled by default.
- Keep human Space names, roles, invitations, and capabilities out of lookup `UserData`, tickets, URLs, metrics labels, and logs.
- Stable Endpoint IDs aid reachability but create cross-Space correlation risk.

Owner discovery contract locked:

- `SpaceAddressRecord { space_id, endpoint_id, sequence, issued_at, expires_at, endpoint_addr }`, signed by the advertised Endpoint identity.
- Forward signed records unchanged only inside the corresponding Space; maintain highest-sequence and TTL checks.
- Expose the per-Space cache through a custom Iroh `AddressLookup`; do not reimplement dialing, NAT traversal, or path selection.
- Address expiry means location unknown, not membership revocation.
- Each Space contains only Space-scoped MA2A Private Relay advertisements; Public Iroh Relay fallback is separate local Runtime connectivity configuration.
- Invite bootstrap must reach at least one member and retrieve current membership state, visible Private Relay advertisements, and signed address records.
- If all cached addresses and all rendezvous paths are unavailable, require a fresh out-of-band invite; automatic rediscovery is impossible by definition.
- Learned revocation removes that Space's cached/forwarded records and private-relay eligibility without invalidating the Endpoint in other Spaces.
- `AddressLookup::resolve(target)` is target-specific: collect fresh authenticated records for that target, merge/deduplicate only their actual `EndpointAddr` data, and return only that reachability to Iroh.
- Never synthesize a target relay address from a Space's Private Relay advertisement or local Public Relay fallback configuration.
- Address records mirror actual current Iroh EndpointData, including only direct addresses and relay/home information through which that Endpoint is really reachable.
- If target records and route knowledge are gone, generic relay lists do not locate the target. Recovery requires target outbound reconnection/republishing, fresh out-of-band bootstrap, or a future rendezvous mechanism.

## Private relay feasibility

- `iroh-relay::server::Server::spawn` and `RelayService::new` support same-process embedding.
- `AccessControl::on_connect` sees an already-authenticated Endpoint ID plus authorization token/request metadata.
- Admission is not tenancy isolation: `Clients` forwards globally by `EndpointId` with no Space field.
- Standard forwarding has no Space context; the relay must not be treated as the Space security boundary.
- Runtime revocation requires application state plus explicit disconnect of admitted sessions.

Owner architecture correction locked:

- An MA2A Private Relay is an optional role hosted by an existing Runtime/Endpoint, configured with `relay_provider.enabled` and a subset `served_spaces` of the hosting Endpoint's active memberships.
- No separate Relay identity, Relay membership object, relay-specific authority, provisioning membership protocol, or external membership feed exists in Phase 1.
- One embedded `iroh-relay` service/listener may serve multiple configured Spaces.
- Private Relay authorization means only “does this authenticated Endpoint ID share at least one served Space with the hosting Endpoint?”
- `private_relay_eligible(E,R) = exists S: active_member(hosting_endpoint(R),S) && relay_enabled_for(R,S) && active_member(E,S)`.
- Relay advertisement remains Space-scoped metadata; sharing infrastructure does not make unrelated Spaces discover one another.
- Receiving MA2A Runtimes independently enforce shared-Space/explicit-grant checks and service-level policy.
- A shared relay may permit transport reachability between admitted endpoints if an Endpoint ID is known; this never authorizes MA2A application access.
- Membership/served-Space changes recompute total eligibility using the hosting Endpoint relationship.
- Disconnect/disallow a shared relay session only when total eligibility transitions from true to false. Revocation in one Space must retain relay access through another still-authorizing Space.
- Operator supplies public listener/DNS/TLS. Automatic reachability detection, TLS/DNS provisioning, provider promotion, Space accounting, quotas, and grant tokens are deferred.
- Public Iroh Relays such as N0 are a separate class: external infrastructure, not MA2A Endpoints or Space members, no MA2A admission, and globally/locally configured as Runtime fallback.
- Keep `private_relay_eligible(E,R)` distinct from `home_relay_compatible(E,R)`.
- `home_relay_compatible(E,R)` requires R to be available/authorized for every active Space in E's memberships. Public Relay fallback is globally compatible when locally enabled.
- Persistent/home candidates are only common compatible Private Relays plus enabled Public Relay fallback—not every Private Relay advertised by any joined Space.
- If a multi-Space Endpoint has no common Private Relay and Public fallback is disabled, report degraded relay reachability; do not promise full relay-backed cold-start reachability.
- Current Iroh preferred/single-home semantics are the reason for this constraint. Opportunistic reuse of another active relay's learned target route may help but is never an MA2A correctness assumption.
- N0 fallback is not stored as Space state.
- Both relay classes are transport only; receiving MA2A authorization remains independent.
- Iroh owns direct/relay path selection; MA2A adds no routing/scoring algorithm.

Primary sources:

- https://github.com/n0-computer/iroh/blob/fddf1a4ce29f92c6651eccff68fb366007b9be7d/iroh-relay/src/server.rs
- https://github.com/n0-computer/iroh/blob/fddf1a4ce29f92c6651eccff68fb366007b9be7d/iroh-relay/src/server/clients.rs
- https://github.com/n0-computer/iroh/blob/fddf1a4ce29f92c6651eccff68fb366007b9be7d/iroh-relay/tests/runtime_auth.rs

## Space security recommendation

- Phase 1 default candidate: owner-signed canonical membership manifests.
- Phase 1 has exactly two private-key classes: one persistent Iroh Endpoint `SecretKey` per Runtime and one separate Space authority/admin signing key.
- Manifest requires Space ID, schema/version, signer/key epoch, strictly increasing sequence, previous hash, membership/revocation state, and durable anti-rollback storage.
- Invitation is an enrollment-request capability, never membership itself; explicit owner approval creates membership.
- Learned revocation terminates that Space's known service authorization and relay admission, while cached valid manifests remain usable during partitions until a newer manifest arrives.
- Phase 1 intentionally has no mandatory freshness lease or group-content key rotation.
- Keep Phase 6 migration hooks for threshold admins/delegated authority; do not implement MLS/multi-admin now.

Owner decision locked for Phase 1:

- Single Space authority/admin key.
- Owner-signed monotonically versioned hash-chained manifests.
- Persist highest observed generation/hash and reject rollback/forks.
- Explicit owner-approved enrollment.
- No mandatory freshness lease; an isolated existing member may continue using its cached valid manifest.
- A learned revocation takes effect immediately for that Space's service sessions and relay admission.
- Authorization teardown is Space-scoped; do not close a shared underlying Iroh connection solely because one Space revoked the peer.
- No Space content-key rotation until MA2A introduces encrypted replicated/group content beyond Iroh transport encryption.
- Portable artifacts use deterministic CBOR with explicit schema/version fields and domain-separated Ed25519 signatures from the separate authority key.
- Invitations are short-lived, single-use, owner-approved Space Invites. Possession permits one enrollment request and never grants membership directly.
- Success, expiry, or explicit cancellation invalidates the invite. Reuse Iroh/`iroh-tickets` bootstrap information for low-level dialing rather than inventing a replacement ticket layer.
- Do not introduce MemberKey, DeviceKey, Human/User key, Account key, or multi-device identity semantics in Phase 1.
- Use deterministic CBOR only for concrete membership/revocation state, invite/enrollment payloads, and `SpaceAddressRecord`; do not build a generic artifact framework.

Standards:

- https://www.rfc-editor.org/rfc/rfc9001.html
- https://www.rfc-editor.org/rfc/rfc9052.html
- https://www.rfc-editor.org/rfc/rfc9420.html

## Interface architecture

- One executable, three roles: daemon, thin CLI client, daemon-served Web UI.
- Current-user-only local IPC: Unix private directory/socket/peer checks; Windows explicit DACL/logon SID and remote-client rejection.
- Local IPC recommendation: cross-platform local socket/named-pipe adapter plus bounded typed frames and versioned envelopes.
- HTTP binds only to loopback, preferably ephemeral port.
- Localhost does not authenticate a caller: require safe bootstrap/session authorization, strict Host/Origin, CSRF, request limits, and no state-changing GET.
- Runtime/SQLite state is authoritative. Web UI loads a snapshot and consumes authenticated best-effort SSE; reconnect, sequence gap, or uncertainty triggers a fresh snapshot.
- Do not persist UI event history or implement exact replay/exactly-once/event-sourced browser state.
- Runtime/UI keep distinct: Private Relay advertisements, Public Relay config, target-specific address records, Private Relay eligibility, and home-relay compatibility.
- UI shows current/home relay class, Space-only availability versus global/home compatibility, target reachability, and explicit degraded relay reachability. It never implies every Space-advertised relay reaches every member.
- Embed release frontend assets and prove execution from a clean directory containing only the binary.

Owner decisions locked:

- Linux, macOS, and Windows; x86_64 and ARM64 where supported by the pinned Rust/Iroh stack.
- `ma2a daemon` is the explicit foreground entry point; Runtime-dependent CLI commands auto-start a missing current-user daemon without elevation.
- No systemd-user/launchd/Windows startup installation in Phase 0/1; headless users may supervise the daemon themselves.
- Daemon exclusively owns the persistent Iroh Endpoint; Unix-domain sockets on Linux/macOS and explicitly ACL-scoped named pipes/equivalent on Windows.
- React + TypeScript + Vite + Tailwind frontend; release assets embedded into `ma2a`.
- Local Runtime protocol is private exact-match; mismatches yield typed upgrade/restart errors and no third-party support promise.
- Browser authentication is a persistent single-user passphrase login, not a one-time URL-carried bootstrap token.
- Store only an Argon2id verifier; use revocable server-side sessions in SQLite and HttpOnly SameSite cookies.
- Authenticated current-user IPC may reset the passphrase and revoke sessions; it must not reveal stored credential material.
- Password reset revokes all existing Web sessions. No pairing codes, URL tokens, OAuth, WebAuthn, 2FA, multiple Web users, or remote Web authentication in Phase 1.
- Initial password creation is only through trusted CLI → Runtime IPC: `ma2a init` or `ma2a ui password set`.
- Without a verifier, the browser shows only setup instructions; it exposes no management state and no unauthenticated password-creation mutation.

## Release scope

- dist/cargo-dist per-target archives.
- SHA-256 checksums, SBOM/provenance, and release smoke tests from clean artifact directories.
- No native installers, package-manager publication, service registration, or auto-update in Phase 0/1.

Primary sources:

- https://docs.rs/interprocess/2.4.3/interprocess/local_socket/
- https://learn.microsoft.com/en-us/windows/win32/ipc/named-pipe-security-and-access-rights
- https://developer.mozilla.org/en-US/docs/Web/Security/Attacks/CSRF
- https://html.spec.whatwg.org/multipage/server-sent-events.html

## Storage recommendation

- Default candidate: bundled SQLite via `rusqlite` for transactionally coupled identity metadata, invite redemption, revocations, policies, migrations, and runtime state.
- Keep private keys outside ordinary database bytes in OS-protected or separately encrypted storage.
- Start with rollback journal + `synchronous=FULL`; one serialized writer.
- Use explicit forward migrations; downgrade through export/import, not blind opening.
- Use SQLite online backup APIs; do not raw-copy a live WAL database.
- Use signed versioned files as export/interchange artifacts, not the primary multi-record transactional store.

Owner decision locked:

- Bundled SQLite/`rusqlite` with schema migrations from the beginning.
- SQLite contains non-secret state and references to protected keys; no plaintext private keys.
- Iroh Endpoint secret: one per Runtime, never synchronized, platform keystore when practical, mode-0600 local fallback for headless/server use; loss means a new Endpoint ID and replacement/revocation of the old one.
- Space authority key: separate from Endpoint keys, never replicated to members, platform protection when available, portable passphrase-encrypted recovery export using established cryptography.
- Recovery scope stays minimal: an explicit encrypted authority-key backup/export may be provided, but no recovery wizard, backup lifecycle manager, automated replication, or polished cross-platform recovery product.
- Signed manifests remain independently portable/exportable.

Primary sources:

- https://crates.io/crates/rusqlite
- https://sqlite.org/transactional.html
- https://www.sqlite.org/atomiccommit.html
- https://www.sqlite.org/backup.html
- https://docs.rs/redb/4.2.0/redb/

## Ecosystem fit

- Candidate now: `irpc`/`irpc-iroh` behind MA2A version/auth adapters; `iroh-tickets` as encoding only; `iroh-ping` diagnostics only; `iroh-metrics` localhost-only.
- Defer: `iroh-gossip`, `iroh-docs`, `iroh-blobs` until authorization/isolation integration tests exist.
- Avoid in Phase 0/1: `iroh-proxy-utils`, RCAN.
- C5 remains a minimal typed dispatch/authorization seam for Echo/Test only. Dynamic plugins, reflection, third-party SDKs, schema languages, and a universal service ecosystem are deferred until real Services define the abstraction.

## QA invariants

- Core/security/state/network tests must be executable and emit machine-readable artifacts; grep/screenshots/self-report are insufficient for those contracts.
- Required hostile coverage: identity restart/corruption; schema versions; invitation leakage/replay; stale/forked manifests; connected revocation; cross-Space leakage; direct/relay transitions; relay admission and forwarding isolation; IPC ACL/single instance; localhost CSRF/origin/session abuse; crash checkpoints; clean-directory single-binary execution.
- Source-driven Iroh connectivity cases: target lookup never synthesizes a Space-advertised relay; incompatible per-Space Private Relays cannot become a multi-Space Endpoint's sole home; common Private Relay may be home; no common relay plus disabled Public fallback reports degraded; opportunistic route reuse is optional only; home-relay changes publish newer signed address records.
- Secrets in evidence must be represented only by hashes/lengths/scopes/redacted forms.
- Owner-selected strategy: hybrid. Use TDD/adversarial evidence for security/state/crypto/authorization/compatibility/crash/network contracts; use ordinary automated acceptance/smoke tests for CSS, layout, non-security UI, reversible frontend components, and packaging cosmetics.
