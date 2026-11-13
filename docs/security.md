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

## Remote Services

A remote operation needs, in one Space both Endpoints currently belong to, the Space owner's signed
grant for the caller and the serving Endpoint's own exposure; grants and exposures from different
Spaces never combine, and the authorizing Space is never revealed. Authorization is read from
durable state for every stream ([authorization](protocol/authorization.md)), and a running
operation ends when a later change withdraws it (for example [hosted calls](protocol/service-v1.md#authorization)
and [Processes](protocol/process-v1.md#withdrawn-authorization)). Retiring a hosted service or an Agent withdraws the
exposures that named it.

Processes, Agents, and hosted services run as the Runtime's OS user. They are not sandboxed from
each other or from that user's files and local API: an Agent or service program can do whatever
its user can. Grant and expose them only to members you would let act as that user within the
granted scope. See [limitations.md](limitations.md#known-trade-offs) for resource bounds that
co-members share.

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
