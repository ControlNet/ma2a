---
slug: ma2a-phase-0-1
status: approved-plan-written
intent: clear
review_required: false
pending-action: execute approved plan in a separate start-work session
approach: Build an acyclic Rust MA2A Core around a persistent zero-or-more-Space Endpoint, Endpoint-centric authorization, private Space-scoped discovery plus bounded control synchronization, Endpoint-hosted MA2A Private Relays with native-or-optional-external TLS, separate Public Iroh Relay fallback, MA2A-filtered/Iroh-selected relay behavior, a minimal Echo/Test seam, and one Runtime API for CLI/Web UI.
---

# Draft: ma2a-phase-0-1

## Components (topology ledger)
<!-- Lock the SHAPE before depth. One row per top-level component that can succeed or fail independently. -->
<!-- id | outcome (one line) | status: active|deferred | evidence path -->
| id | outcome (one line) | status | evidence path |
| --- | --- | --- | --- |
| C1 | Define the Rust workspace, Universe/Space/Endpoint ontology, versioned state/config contracts, and single-binary composition without a global Universe registry. | active | User correction sections 1 and 15; `/home/zhixi/GitRepos/ma2a/README.md:1` confirms a greenfield repository. |
| C2 | Keep one persistent Iroh Endpoint alive behind a cross-platform local Runtime IPC boundary with stable identity and recoverable state. | active | User brief, sections 2 and Phase 1. |
| C3 | Implement private Space lifecycle, Endpoint-to-Space memberships, endpoint-centric authorization paths, signed membership/revocation, Space-scoped discovery, and bounded authenticated control-state synchronization without implicit leakage. | active | User correction sections 1-3, 5-8, and 16; focused review correction 1. |
| C4 | Establish direct connectivity plus two explicit relay classes: Endpoint-hosted Space-advertised MA2A Private Relays and globally configured Public Iroh Relay fallback; MA2A filters safe candidates while Iroh selects the effective home/path. | active | User focused relay correction sections 1-4 and 9; focused review corrections 2-3. |
| C5 | Provide only the clean internal dispatch seam needed for a minimal authorized encrypted Echo/Test service, without designing a final plugin ecosystem. | active | User correction sections 9, 15, and 16. |
| C6 | Expose one Runtime API through CLI and localhost-only Web UI, stream topology/state, and ship all Phase 0/1 roles in one `ma2a` binary. | active | User brief, sections 2-3 and Phase 1. |

## Conceptual model (locked)

- **Universe** is the conceptual global MA2A world containing all Spaces and Endpoints. It has no `UniverseId`, global registry, join operation, authority, or universally replicated database. Each participant sees only the subset exposed through its Spaces and future explicit grants.
- **Space** is the privacy, visibility, membership, and trust/policy boundary. Nothing crosses a Space boundary implicitly: Endpoint/service discovery, metadata, relay/address advertisements, events, or future service visibility.
- **Endpoint** is the persistent Iroh identity owned by one Runtime and exists independently of membership. An Endpoint belongs to `0..N` Spaces; creating/leaving/revocation changes membership relationships, never the Endpoint identity.
- Persist `Endpoint { endpoint_id, runtime_metadata }` separately from `SpaceMembership { space_id, endpoint_id, ... }`. Never embed `space_id` into Endpoint identity.
- A zero-Space Endpoint is a valid running Runtime with a stable Endpoint ID, but normal remote MA2A service/discovery/metadata access fails closed. Only the narrowly scoped invite/enrollment protocol is available without a shared Space.

## Open assumptions (announced defaults)
<!-- Record any default you adopt instead of asking, so the user can veto it at the gate. -->
<!-- assumption | adopted default | rationale | reversible? -->
| assumption | adopted default | rationale | reversible? |
| --- | --- | --- | --- |
| Rust baseline | Pin stable Rust 1.91 or newer compatible with `iroh 1.1.x`; do not use the repository machine's unpinned nightly as the project contract. | Iroh 1.1.0 declares Rust 1.91; a repository pin is needed for reproducible CI. | yes |
| Iroh lifecycle | One daemon-owned, long-lived `Endpoint` using a persisted `SecretKey`; all CLI/UI roles are clients of the Runtime. | Preserves `EndpointId`, prevents duplicate owners, and matches Iroh Router's multi-ALPN model. | no at public architecture level |
| Iroh composition | Do not use `presets::N0` unchanged. Compose the Endpoint with MA2A's private Space-aware `AddressLookup`, explicit relay configuration, and optional N0 public relays only as transport fallback. | N0 public relay use is distinct from public DNS/Pkarr address publication; accidental public publication must be difficult by default. | no at Phase 1 privacy contract |
| Private discovery | Resolve a target only from fresh signed target-specific `SpaceAddressRecord` data; Private Relay advertisements and Public Relay configuration never synthesize target relay addresses. | Iroh `AddressLookup` resolves actual target reachability, not generic local relay availability. | no at correctness boundary |
| MA2A Private Relay | An existing Runtime/Endpoint may enable one embedded relay and serve a configured subset of Spaces that hosting Endpoint actively belongs to; native `iroh-relay` TLS with operator-supplied cert/key and optional compatible external TLS termination are both allowed; no separate relay identity, membership feed, or authority exists. | Preserves the single-binary Endpoint-hosted role without adding DNS/certificate automation or a mandatory sidecar. | no at Phase 1 model |
| Public Iroh Relay | Public relay fallback such as N0 is external global transport infrastructure configured locally per Runtime, not a Space member/resource/advertisement. | Public relay reachability never implies MA2A discovery, trust, or authorization. | yes, local connectivity policy |
| Home relay compatibility | MA2A supplies Iroh only Private Relays serving every active Space of the local Endpoint plus enabled Public Relay fallback; Iroh performs preferred/home relay and path selection. | Preserves the approved responsibility boundary and excludes Space-specific-only relays from the generic candidate map. | no for current Iroh Phase 1 |
| RPC boundaries | Use MA2A versioned contracts; use `irpc-iroh` only behind an adapter for remote Iroh RPC, and a bounded local IPC framing layer for daemon clients. | `irpc` supports streaming but explicitly does not provide MA2A compatibility/version policy. | yes |
| Remote addressing | Normal commands address a target Endpoint, not a Space. The receiver derives all shared Spaces and accepts only when one complete independent Space policy authorizes the full operation. | Keeps Space as internal authorization context and prevents privilege composition across Spaces. | no at user/API semantics |
| Local interface | Current-user-only IPC; HTTP binds to loopback on an ephemeral port; authenticated SSE is the first event transport; WebSocket only if bidirectional streaming becomes necessary. | Minimizes attack surface and avoids fixed-port collisions while keeping UI events simple. | yes |
| UI baseline | Minimal technical operations dashboard, accessible contrast/focus/reduced-motion behavior, embedded release assets, no source-tree dependency at runtime. | Fits the local topology/security product and single-binary requirement. | yes |
| UI synchronization | Runtime/SQLite state is authoritative; browser loads a snapshot and consumes best-effort authenticated SSE. Any reconnect/gap/uncertainty triggers a fresh snapshot. | Avoids durable UI event history, replay, and event-sourcing over-design. | yes |
| Deferred ecosystem | Defer `iroh-gossip`, `iroh-docs`, and `iroh-blobs`; avoid `iroh-proxy-utils` and RCAN in Phase 0/1. | None provides MA2A Space authorization by itself; several add maturity or scope risk. | yes |

## Findings (cited - path:lines)
- The repository is greenfield: the root contains only `.git/`, `.codegraph/`, and `README.md`; the README is one line (`/home/zhixi/GitRepos/ma2a/README.md:1`). There are no manifests, source files, tests, CI workflows, project rules, or prior `.omo/knowledges/` artifacts to preserve.
- The CodeGraph index returned no relevant implementation symbols, consistent with the empty source tree.
- Git history contains only initial commit `4f252185a6e7c7013f7b807dd59dea54ae32060e`; `master` and `origin/master` are aligned. Untracked `.omo/` planning/session files must not be deleted or overwritten outside the planning workflow.
- Iroh Core was verified against `iroh 1.1.0`, commit `fddf1a4ce29f92c6651eccff68fb366007b9be7d`. Stable public contracts cover persistent `SecretKey`/`EndpointId`, ALPN routing, connect/accept, path snapshots/events, RTT, relay-map mutation, and graceful close. `stable_id()` is connection-lifetime-only; path identity and path selection are not stable across reconnects.
- `Endpoint::insert_relay` and `Endpoint::remove_relay` update runtime relay configuration but do not guarantee synchronous migration/draining of existing peer connections.
- Current Iroh exposes preferred/home-relay behavior rather than a Phase 1 guarantee that one Endpoint remains persistently registered at every eligible relay. Opportunistic reuse of another active relay's learned route is an optimization, not a correctness contract.
- `AddressLookup` is target-specific: generic Private Relay availability or locally configured Public Relay fallback does not prove that a remote target is attached to those relays. Only the target's actual signed Endpoint reachability belongs in `resolve(target)`.
- Default N0 DNS/Pkarr address lookup publishes addressing metadata. Phase 1 must not instantiate `presets::N0` unchanged: MA2A composes private Space-aware lookup and explicit relays, while optional N0 public relays remain transport-only fallback.
- `iroh-relay::AccessControl` authenticates/admit connections, while the standard `Clients` registry forwards by destination `EndpointId` without Space context. A shared relay is therefore not a Space security boundary, but it is valid shared transport infrastructure when MA2A separately enforces discovery, Space authorization, and service policy at each receiving Runtime.
- Phase 1 needs only two private-key classes: the per-Runtime Iroh Endpoint `SecretKey` and the separate Space authority/admin signing key. Undefined Member, Device, Human, or Account identity keys are explicitly deferred.
- Current-user-only local IPC is not automatic: Unix requires a private directory, socket permissions, ownership/peer checks, and safe stale-socket handling; Windows requires an explicit current-user/logon-SID DACL and remote-client rejection.
- Localhost is not authentication. The locked Web UI model is a single-user password, Argon2id verifier, server-side revocable session, HttpOnly/SameSite cookie, strict Host/Origin checks, CSRF protection, bounded requests, and no state-changing GETs.
- Storage research recommends bundled SQLite through `rusqlite` for atomic invitation redemption, revocation, policy, and migration state; signed files remain useful as export/interchange artifacts. SQLite keys are not secret storage, and live backups must use the online backup API rather than raw copies.
- Heavy TDD/adversarial evidence is reserved for Core/security/state/network contracts. Reversible CSS/layout/frontend presentation and packaging cosmetics use ordinary automated acceptance/smoke tests.

## Decisions (with rationale)
- Preserve the six-component topology C1-C6 and implement it as an acyclic graph: `C1 -> {C2,C3}`, `{C2,C3} -> C4`, `{C2,C3,C4} -> C5`, `{C2,C3,C4,C5} -> C6`.
- Keep C3 authoritative for Space authorization, C4 authoritative for connectivity/path selection, C5 authoritative for service dispatch, and C6 dependent only on the Runtime API/event projection rather than component internals.
- Separate transport identity from application authorization: an authenticated Iroh `EndpointId`, ALPN, relay admission, invite, or discovery topic never independently proves Space membership.
- Treat security failures as fail-closed and require machine-readable evidence artifacts; grep hits, screenshots, and worker self-report are not acceptance evidence.
- Normal remote requests identify the target Endpoint. The receiver computes `shared_spaces = memberships(caller) ∩ memberships(target)` internally and evaluates each shared Space independently.
- Authorization succeeds iff at least one single Space independently authorizes the complete requested operation. A deny in another Space does not cancel an independent allow, and partial permissions from different Spaces must never be composed. Record `authorized_via` internally for audit/debug only; it is not normally user supplied.
- If there is no shared Space, normal MA2A discovery, metadata, and service requests deny by default. The only Phase 1 exception is a narrowly scoped valid invite/enrollment flow that cannot invoke normal Services.
- Phase 1 Space authority is a single owner/admin signing key, separate from every Iroh Endpoint key. Membership uses owner-signed, monotonically versioned, hash-chained manifests; each Runtime durably persists its highest observed generation/hash and rejects rollback/forks.
- Joining uses a short-lived, single-use, owner-approved Space Invite. Possession authorizes one enrollment request, not membership; success, expiry, or cancellation permanently consumes/invalidates the invite. Reuse Iroh/`iroh-tickets` bootstrap information for low-level dialing.
- A newly learned valid revocation applies immediately to that Space's authorization, service sessions, and relay admission. Do not blindly close the underlying Iroh connection when the same endpoints still share another valid Space; authorization and teardown are Space-scoped.
- Existing members synchronize signed Space control state through bounded authenticated `ma2a/control/1` pull plus opportunistic push. Forwarded manifests, SpaceAddressRecords, and PrivateRelayAdvertisements remain trusted only through their original signatures and existing generation/sequence/hash/membership validation; no generic replication framework is introduced.
- Do not add Space content-key rotation in Phase 1. Iroh already provides transport E2E encryption; group-content epochs become relevant only if MA2A later adds Space-level encrypted replicated content.
- Durable state uses bundled SQLite/`rusqlite` with schema migrations from the first version. SQLite stores non-secret manifests, memberships, policies, service/relay metadata, and protected-key references; signed manifests remain independently portable/exportable.
- The persistent Iroh Endpoint secret is one per Runtime, never auto-synchronized, and may sign Endpoint-owned ephemeral records such as `SpaceAddressRecord`. Prefer platform protection with an owner-only local file fallback. Loss creates a new Endpoint ID and requires revocation/replacement; Space continuity does not depend on recovering it.
- The Space authority/admin signing key is the only other Phase 1 private-key class. It signs membership state, revocations, and enrollment authorization; it is independently protected and never replicated to members. A minimal explicit encrypted backup/export is allowed, but recovery wizards, key replication, and backup lifecycle management are out of scope.
- Test strategy is hybrid: TDD plus adversarial evidence for security, cryptographic, storage, authorization, compatibility, crash, IPC, and network contracts; tests-after with ordinary automated acceptance/smoke QA for reversible UI and packaging adapters.
- Target Linux, macOS, and Windows, with x86_64 and ARM64 release targets wherever the pinned Rust/Iroh dependency stack supports them. CI covers major OS families natively where practical without requiring every host/architecture combination to have a hosted runner.
- `ma2a daemon` is the explicit foreground/debug entry point. Any CLI operation requiring Runtime state auto-starts a missing per-user daemon without elevation; headless deployments may run/supervise `ma2a daemon` explicitly. Phase 0/1 does not install native login/boot services.
- The auto-started/current-user daemon is the sole persistent Iroh Endpoint owner. Linux/macOS clients use a Unix-domain socket; Windows uses a user-scoped named pipe or equivalent explicitly ACL-restricted IPC.
- The embedded Web UI uses React + TypeScript + Vite + Tailwind, with built assets embedded into the release binary and no Node/runtime frontend dependency for end users.
- The local Runtime IPC/API is private and exact-version-match in Phase 0/1. CLI/UI/daemon mismatches return a typed restart/upgrade error; no third-party compatibility promise is made yet.
- An **MA2A Private Relay** is an optional role of an existing Runtime/Endpoint: `relay_provider.enabled` plus `relay_provider.served_spaces`. The hosting Endpoint must actively belong to every served Space. Do not create a separate Relay identity, Relay membership object, provisioning membership protocol, external membership feed, or relay-specific Space authority.
- One embedded `iroh-relay` listener may serve multiple configured Spaces. Its effective admission set is the union of active Endpoint IDs in those served Spaces; clients provide no Space context when connecting.
- Private-relay eligibility is `private_relay_eligible(E,R) = exists S: active_member(hosting_endpoint(R),S) && relay_enabled_for(R,S) && active_member(E,S)`. Recompute after membership or served-Space changes and disconnect only when eligibility transitions from true to false.
- Private Relay advertisements remain Space-scoped metadata. The physical listener may be shared, but only members of each served Space learn that it is available for that Space.
- A **Public Iroh Relay** such as N0 is external transport infrastructure: not an MA2A Endpoint, not a Space member, unaware of membership, and not advertised as private Space infrastructure. Every Runtime may use its locally configured Public Relay fallback independently of Space policy.
- Keep `private_relay_eligible(E,R)` separate from `home_relay_compatible(E,R)`. The first means E may use R through at least one served Space; the second means R is safe as E's persistent advertised home across every active Space membership.
- `home_relay_compatible(E,R) = for every active Space S in memberships(E), R is advertised/authorized for S`. Public Iroh Relays are globally compatible when local fallback is enabled.
- `persistent_home_relay_candidates(E)` contains only home-compatible Private Relays plus enabled Public Relay fallback. Do not feed every Space-advertised Private Relay to Iroh as an equivalent persistent/home candidate.
- MA2A computes that candidate RelayMap and supplies it to Iroh; Iroh—not MA2A—probes and selects the actual preferred/home relay and connection path. Desired candidate state and observed effective Iroh state remain separate.
- If a multi-Space Endpoint has no common Private Relay and Public fallback is disabled, report **degraded relay reachability** honestly. Direct paths may still work, but full relay-backed cold-start reachability is not guaranteed.
- Domain/storage/Runtime adapters must keep semantically distinct: Private Relay advertisement, Public Relay config, target-specific `SpaceAddressRecord`, Private Relay eligibility, and home-relay compatibility. They need not all become public API types, but no implementation path may conflate them.
- Sharing either relay class creates no Space or service trust. A receiving Runtime still derives shared Spaces and denies ordinary access when none independently authorizes the request.
- Browser login is single-user passphrase authentication. Store only an Argon2id password verifier; persist revocable server-side sessions in SQLite; issue HttpOnly SameSite cookies; authenticated current-user IPC can reset the passphrase and revoke all sessions without exposing the stored verifier.
- Initial password creation is trusted CLI-to-Runtime IPC only: `ma2a init` may set it, and `ma2a ui password set` sets/resets it explicitly. If no verifier exists, the browser exposes only an instruction to run that command—no management state and no unauthenticated browser mutation endpoint. Reset replaces the verifier and revokes every existing session.
- Release scope is per-target archives generated by dist/cargo-dist with SHA-256 checksums, SBOM/provenance, and clean-artifact smoke tests. Phase 0/1 excludes native installers, package-manager publication, and auto-update.
- Use deterministic CBOR only for the concrete signed Phase 1 structures that need it: membership state/revocation, Space Invite/enrollment payload, and `SpaceAddressRecord`. Do not create a generic portable-artifact or serialization framework.
- Each Runtime signs `SpaceAddressRecord { space_id, endpoint_id, sequence, issued_at, expires_at, endpoint_addr }` with the advertised Endpoint key. Records are forwarded unchanged only within the corresponding Space, cached with highest-sequence/TTL rollback checks, and exposed to Iroh through a custom Space-aware `AddressLookup`.
- New records must mirror actual current Iroh EndpointData when direct addresses, preferred/home relay, or relevant reachability changes. Never advertise a relay merely because the local Endpoint is authorized to use it.
- Publish only reachability appropriate to each Space. A globally usable Public Relay home, or a common Private Relay serving all joined Spaces, may be published across those Spaces; avoid choosing a private home relay inaccessible to members of another active Space.
- Address-record expiry means location unknown, not membership revocation. On learned revocation, discard and stop forwarding that member's records for the affected Space, remove private-relay eligibility, and reject Space operations without invalidating other Spaces.
- Each Space stores only its Space-scoped **Private Relay Advertisements**. Public Relay fallback is separate local Runtime connectivity configuration, not part of Space state. Invites bootstrap at least one member plus current membership state and visible Private Relay advertisements.
- Custom `AddressLookup::resolve(target)` collects fresh authenticated `SpaceAddressRecord`s for that target from shared Spaces, merges/deduplicates only the target-specific `EndpointAddr` information they contain, and returns only that actual reachability to Iroh.
- Never synthesize `TransportAddr::Relay(R)` for a target merely because R is advertised by a shared Space or configured as Public Relay fallback. A relay URL belongs in target lookup only when the target's valid record states that it is currently reachable through that relay.
- Private Relay advertisements and Public Relay configuration separately control the local Endpoint's usable infrastructure and persistent/home candidate set; they do not locate a remote Endpoint.
- Cold-start resolution is fresh target-specific signed records followed by Iroh dialing/NAT/direct upgrade/path selection using those records. Iroh may opportunistically reuse learned relay routes, but correctness must not depend on it. If target records and route knowledge are gone, recovery requires target outbound reconnection/republishing, fresh out-of-band bootstrap, or a future rendezvous mechanism.
- Build the Iroh Endpoint from explicit MA2A components rather than unchanged `presets::N0`: private `AddressLookup`, Space-scoped Private Relay advertisements, separate local Public Relay fallback, and no public DNS/Pkarr publication by default.
- C5 defines only a small typed internal dispatch/authorization seam and one encrypted Echo/Test request-response protocol. Do not add dynamic loading, reflection, third-party SDKs, schema languages, plugin packages, or a universal service registry.
- The Web UI treats Runtime/SQLite as authoritative: fetch a state snapshot, consume authenticated best-effort SSE, and fetch a fresh snapshot after reconnect, gap, or uncertainty. Do not persist UI event history or implement exact replay/exactly-once semantics.
- Runtime state and Web UI distinguish Private Relay advertisement, Public Relay configuration, target-specific address records, Private Relay eligibility, and home-relay compatibility. UI shows current/home relay class, reachability status, Space-only availability versus home compatibility, and degraded relay reachability without implying every advertised relay reaches every member.

## Scope IN
- Rust workspace/toolchain/dependency policy, locked Universe/Space/Endpoint ontology, and single `ma2a` binary composition.
- Versioned domain/config/state/RPC/event contracts and typed error model.
- One persistent daemon-owned Iroh Endpoint that validly belongs to `0..N` Spaces, with identity persistence and path/RTT telemetry.
- Space create/invite/join/approve/revoke flows; short-lived single-use invites; endpoint-centric independent authorization paths; zero-Space default deny; anti-replay/rollback state; strict multi-Space separation.
- Direct connectivity; Endpoint-hosted Private Relay Provider configuration/advertisement/admission; separate local Public Relay fallback; target-only AddressLookup resolution; home-relay compatibility/degraded-state derivation; and Iroh path/RTT state.
- Minimal typed internal dispatch seam and encrypted Echo/Test service only.
- Cross-platform current-user local IPC, CLI, localhost-only password-authenticated Web UI, snapshot + best-effort SSE projection, embedded frontend assets.
- Hybrid testing: TDD/adversarial QA for Core/security/state/network contracts; ordinary automated acceptance/smoke tests for reversible UI and packaging work.

## Scope OUT (Must NOT have)
- No global Universe registry, `UniverseId`, Universe join operation, global membership authority, or universally replicated Universe database.
- No Endpoint identity containing a Space ID; no requirement that an Endpoint belongs to any Space.
- No normal user requirement to provide `--space`; no cross-Space privilege composition or global-deny semantics.
- No multi-admin/delegated administration or MLS implementation in Phase 0/1; formats may reserve a migration path only.
- No MemberKey, DeviceKey, Human/User identity key, Account key, or undefined multi-device identity hierarchy.
- No claim that default Iroh discovery, gossip topics, endpoint tickets, relay admission, or encrypted transport equals Space authorization/privacy.
- No unchanged `presets::N0` configuration that enables public DNS/Pkarr address publication.
- No shared Space secret as the sole membership authority.
- No claim that sharing a relay creates shared Space trust, discovery rights, or service authorization.
- No generic Relay identity/membership object, relay-specific Space authority, provisioning membership protocol, or external relay membership feed.
- No modeling Public Iroh Relays as MA2A Endpoints, Space members, or Space-scoped relay advertisements.
- No generic `Space Relay Set` that mixes Private Relay advertisements with Public Relay fallback.
- No MA2A NAT traversal, path selection, relay scoring, or routing algorithm.
- No synthesized target relay address from generic Private Relay advertisements or Public Relay configuration.
- No correctness dependency on Iroh opportunistically reusing a relay that previously learned a route to the target.
- No reusable/fleet/automated invites or long-lived enrollment-link lifecycle.
- No dynamic plugin loading, service reflection, third-party Service SDK, generic schema language, plugin packaging, operation-descriptor ecosystem, or final universal service abstraction.
- No generic portable MA2A artifact/serialization framework beyond concrete signed Phase 1 structures.
- No durable UI event history, exact replay, exactly-once UI events, or browser-side replicated/event-sourced state.
- No polished recovery wizard, automated key replication, backup manager, or cross-platform recovery product.
- No public/remote Web UI, wildcard CORS, unauthenticated localhost mutations, or URL/query/fragment bearer credentials.
- No Files, Exec, Shell, Process, Agent, A2A, Proxy, cross-Space Grants, or advanced distributed orchestration.
- No automatic Relay Provider promotion, automatic DNS/TLS provisioning, advanced relay scoring/routing, or public/remote relay-management plane.
- No `iroh-proxy-utils`, RCAN, self-update mechanism, native OS service installer, or mandatory package-manager distribution unless the owner expands scope.
- No unstable Iroh net-report/custom-transport APIs in core contracts without an isolated adapter and explicit approval.
- No source-tree frontend assets or runtime network fetch required by the release binary.

## Source-driven connectivity acceptance

- **A — Target-specific relay resolution:** when a Space advertises R1 but target B's signed record states actual reachability through R2, `AddressLookup(B)` returns B's R2 data and never synthesizes R1.
- **B — Multi-Space incompatible Private Relays:** B belongs to A+B; R-A serves only A and R-B only B; with Public fallback enabled, neither Private Relay may become B's sole persistent/home relay and B remains relay-reachable through a compatible Public fallback.
- **C — Common Private Relay:** B belongs to A+B and R serves both; R is home-compatible and relay-backed connectivity succeeds for authorized peers from both Spaces.
- **D — No common relay:** B belongs to A+B, no common Private Relay exists, and Public fallback is disabled; Runtime reports degraded relay reachability and never claims complete cold-start reachability.
- **E — Opportunistic route reuse:** exercise Iroh reusing another active relay's learned route and verify it may improve connectivity, while tests and product guarantees remain correct if reuse does not occur.
- **F — Home relay change:** when Iroh changes B's actual preferred/home relay or relevant EndpointData, B emits a newer signed `SpaceAddressRecord`; peers replace stale target-specific lookup state using sequence/TTL validation.

## Phase 1 success model

1. A user can create and run a persistent Endpoint with zero Space memberships.
2. A zero-Space Endpoint exposes no normal MA2A discovery, metadata, or Services to arbitrary remote peers.
3. A user can create a private Space.
4. Another persistent Endpoint can submit one enrollment request through a short-lived single-use invite and join only after owner approval.
5. An Endpoint can belong to multiple Spaces without changing Endpoint ID.
6. Space visibility/discovery remains private and nothing crosses a Space boundary implicitly.
7. Signed Space-scoped `SpaceAddressRecord`s provide authenticated target-specific reachability, and `AddressLookup` returns only the target's actual recorded EndpointData.
8. Direct connectivity works whenever Iroh can establish it.
9. Private Relay advertisements define Space-scoped available infrastructure, not arbitrary target locations; Endpoint-hosted providers retain union-derived admission and may serve multiple Spaces.
10. Separately configured N0 Public Relay fallback is globally home-compatible when enabled, without becoming Space state or enabling public MA2A discovery.
11. Multi-Space Endpoints use only persistent/home relay candidates compatible with every active membership: a common Private Relay or enabled Public fallback; otherwise Runtime reports degraded relay reachability.
12. Iroh owns actual connection/path selection and direct upgrades; MA2A neither synthesizes target reachability nor depends on opportunistic learned-route reuse.
13. Revocation affects only the relevant Space, removes Private Relay access only when total eligibility becomes false, and triggers home-compatibility/degraded-state recomputation without changing Endpoint ID.
14. Authorized members complete the minimal encrypted Echo/Test request-response; no-shared-Space and unauthorized requests fail closed.
15. CLI and Web UI both operate exclusively through the persistent Runtime.
16. Web UI password is initialized/reset only through trusted local IPC, and the authenticated UI displays visible Spaces, Endpoints, membership, target reachability, current/home relay class, eligibility/compatibility/degraded state, Connections/Paths/RTT, and Echo/Test through snapshot + best-effort SSE projection.
17. Existing members eventually learn newer valid manifests/revocations, fresh SpaceAddressRecords, and fresh PrivateRelayAdvertisements through bounded authenticated control synchronization without cross-Space leakage or weakening original-signature validation.

## Open questions
- Resolved: Space authority/revocation contract; durable storage and key recovery boundary; hybrid test strategy.
- Resolved: supported OS/architecture matrix and daemon startup model; React/Vite/TypeScript frontend; private exact-match Runtime protocol.
- Resolved: shared multi-Space relay transport contract; persistent local Web login; archive/checksum/SBOM/provenance releases.
- Resolved: single-user Argon2id local passphrase, server-side revocable sessions, and IPC-based reset/revoke recovery.
- Resolved: deterministic CBOR for concrete signed structures; short-lived single-use owner-approved invites; signed Space-scoped address records + custom Iroh AddressLookup + durable relay bootstrap.
- Resolved by correction pass: Universe ontology, zero-Space Endpoints, endpoint-centric authorization, relay eligibility recomputation, two-key identity model, unchanged-N0 prohibition, minimal service seam, simple password login/SSE, proportional hybrid QA.
- Resolved by final relay pass: Endpoint-hosted MA2A Private Relay versus external Public Iroh Relay, separate data/visibility/admission semantics, concrete cold-start resolution hierarchy, and CLI-only initial Web password setup.
- Resolved by source-driven Iroh pass: target-only AddressLookup, no synthesized relay addresses, separate relay eligibility/home compatibility, single-home candidate intersection, degraded multi-Space reachability, and six explicit integration tests.
- Resolved by focused plan review: add `ma2a/control/1` ongoing control-state synchronization; make MA2A filter the RelayMap while Iroh selects the effective home/path; support native embedded relay TLS with optional external termination; restore/evidence x86_64+ARM64 Linux/macOS/Windows release coverage.
- Resolved by final focused corrections: control sync actively dials known reachable peers without a pre-existing connection; enrollment validates a complete contiguous manifest chain; Endpoint-owned address/relay objects explicitly bind signer identity to the claimed Endpoint/provider.
- No blocking ambiguity remains.

## Approval gate
status: approved-final
approach: Implement C1-C6 in dependency waves: ontology/contracts/toolchain first; persistent zero-Space Runtime and Space membership/authorization in parallel; target-specific discovery, embedded Private/Public relay configuration, bounded authenticated control synchronization, MA2A-filtered/Iroh-selected relay behavior, and minimal Echo next; CLI/Web UI—including sync/relay observability and trusted IPC password initialization—and architecture-accounted packaging last. Apply heavyweight adversarial evidence only to Core/security/state/network contracts and ordinary automated QA to reversible UI/packaging work.
next-action: The final plan at `.omo/plans/ma2a-phase-0-1.md` is explicitly approved. Execute it only in a separate worker session with `$start-work ma2a-phase-0-1` and optional worktree/PR shipping flags.
<!-- When exploration is exhausted and unknowns are answered, set status: awaiting-approval. -->
<!-- That durable record is the loop guard: on a later turn read it and resume at the gate instead of re-running exploration. -->
