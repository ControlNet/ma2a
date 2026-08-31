# Security Boundaries

## Trust Boundaries

- The operating-system current-user boundary protects the state directory, protected key files,
  daemon IPC endpoint, and loopback Web control plane.
- Space signatures, contiguous manifest history, explicit membership, capability, revocation, and
  freshness checks authorize MA2A operations.
- Iroh authenticates the remote Endpoint transport identity. MA2A still requires exact target
  identity and Space authorization after transport establishment.
- Relay admission and reachability are transport concerns. They do not grant Space membership or
  application authorization.
- Release checksums detect changed bytes; GitHub OIDC attestations bind archive digests to the
  repository commit and `.github/workflows/release.yml`; SBOM attestations bind dependency metadata
  to the same archive subject.

## Threat Model

MA2A fails closed for malformed or ambiguous local API input, cross-user IPC, non-loopback Web
peers, hostile Host/Origin/CSRF/session input, enrollment replay, signature mismatch, stale or forked
control state, cross-Space data, Endpoint identity mismatch, and relay provider mismatch. Secret
keys, passwords, invitation tickets, raw session bearers, and TLS key contents must not enter argv,
URLs, logs, release metadata, or repository files.

The system does not protect a Runtime after the operating-system user account or process is fully
compromised. It does not provide anonymity, traffic-flow concealment, hardware-backed key custody,
automatic certificate issuance, or recovery from lost private keys.

## Web Boundary

The UI binds only to an OS-selected loopback port. Authentication uses an Argon2id password
verifier, host-only strict cookies, independent CSRF state, exact same-origin checks, bounded login
attempts, and no permissive CORS. Static assets are compiled into the binary; the release smoke test
fetches the HTML and referenced JS/CSS from the extracted executable with no source asset directory.

## Release Verification

Treat an archive as trusted only after both digest and provenance verification. A valid checksum
without provenance proves integrity relative to the sidecar, not origin. A valid provenance
attestation without comparing the expected repository and signer workflow proves the wrong policy.
Use both constraints shown in [quickstart.md](quickstart.md).
