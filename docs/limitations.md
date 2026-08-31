# Phase 1 Limitations and Exclusions

MA2A Phase 1 intentionally provides one current-user binary, one persistent Endpoint per Runtime,
zero or more Spaces, bounded signed control synchronization, Endpoint-addressed Echo, loopback Web
administration, and operator-configured relays.

It does not provide:

- installers, MSI/pkg/deb/rpm packages, Homebrew, Winget, package-manager publication, auto-update,
  or OS login-service installation;
- a Universe registry, public discovery service, public Web UI, generic plugin/service framework,
  pub-sub, CRDT, or gossip subsystem;
- a second MA2A identity key class, automatic relay promotion, MA2A-owned preferred/home relay
  selection, path scoring, or a universal relay-backed reachability guarantee;
- automatic TLS certificate issuance/renewal or a mandatory external TLS sidecar;
- raw live SQLite copy as backup, automatic private-key export, cloud escrow, key recovery, or
  cross-device secret replication;
- silently assumed ARM64 support. Deferred targets and their reproducible blockers are listed in
  [platform-support.md](platform-support.md).

Transient learned Iroh routes may improve a connection but are not persisted authorization facts
and are never required for correctness. Observed online state is diagnostic, expires, and is cleared
on restart until the new process observes current state.
