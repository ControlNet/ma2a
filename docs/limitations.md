# Limitations and Exclusions

MA2A provides one current-user binary, one persistent Endpoint per Runtime, zero or more Spaces,
bounded signed control synchronization, Endpoint-addressed Echo, Web administration over HTTP
(loopback by default), operator-configured relays, and the service families of Phases 2 to 5:
remote Files, Transfer, Exec, Shell, and Proxy; durable Processes; Agents with an A2A v1.0
adapter; and hosted third-party services.

It does not provide:

- installers, MSI/pkg/deb/rpm packages, Homebrew, Winget, package-manager publication, auto-update,
  or OS login-service installation;
- a Universe registry, public discovery service, public Web UI, pub-sub, CRDT, or gossip
  subsystem. Hosted services are discovered only per Endpoint, by members of a shared Space;
- a second MA2A identity key class, automatic relay promotion, MA2A-owned preferred/home relay
  selection, path scoring, or a universal relay-backed reachability guarantee;
- automatic TLS certificate issuance/renewal or a mandatory external TLS sidecar;
- raw live SQLite copy as backup, automatic private-key export, cloud escrow, key recovery, or
  cross-device secret replication;
- isolation between the programs one OS user runs: Processes, Agents, and hosted services run as
  that user and can use everything it can, including the local API (see [security.md](security.md));
- silently assumed ARM64 support. Deferred targets and their reproducible blockers are listed in
  [platform-support.md](platform-support.md).

Transient learned Iroh routes may improve a connection but are not persisted authorization facts
and are never required for correctness. Observed online state is diagnostic, expires, and is cleared
on restart until the new process observes current state.

## Known Trade-offs

- **Request fences grow.** Every admitted local mutation leaves a permanent RequestId fingerprint
  (no command body) so that the request can never run twice, even after its cached response was
  evicted. Fences have no expiry; their growth follows the number of admitted mutations,
  including remote streams and Agent tool calls
  ([local-api-v1](protocol/local-api-v1.md#mutation-replay)).
- **Service discovery admits every co-member.** Discovery must answer any member of a shared Space,
  so the coarse admission of the Service family checks membership only. A member without the
  `service` grant can therefore hold Service-family stream slots (16 per peer, 64 per Runtime)
  until the 15 s deadline for its `open`, delaying other members' discovery and hosted calls on
  that Runtime. It learns nothing it is not granted.
- **The service SDK builds the Runtime.** `ma2a-service` uses the IPC client and local API types
  of `ma2a-runtime`, so a third-party service compiles the Runtime's dependency graph. A smaller
  shared client crate would remove that coupling; it is not split out yet.
- **Local commands are serialized.** Every local API command except the handshake and the
  Web UI lifecycle runs under one Runtime-wide replay lock, reads included. A slow local mutation
  delays other local commands; it never changes their outcome. Stream and hosted-service upgrades
  do not take the lock.
- **Control synchronization needs a fresh address.** A peer is dialed for control only through a
  signed address record that is still valid, which lasts at most ten minutes. When two members of
  a Space are both offline past that window and no other member passes either a newer record,
  neither can find the other again: periodic rounds skip the peer, and
  `ma2a space sync now --endpoint` for it fails. Dialing control with an expired record as a hint
  is not implemented, because it would have to decide which of the record's relays are still
  authorized.
