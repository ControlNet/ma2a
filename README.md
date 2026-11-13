# ma2a

MA2A is a current-user, endpoint-centric Runtime for independently authorized Spaces. One binary
owns one persistent Iroh Endpoint identity, a private local control plane, and an authenticated
loopback Web console. Release archives contain the executable and operator documentation only; the
frontend is embedded and requires no source tree or external Web asset service.

Within a Space, an Endpoint can serve the members its owner grants: remote Files and verified
transfers, Exec, Shell, and TCP Proxy; durable Processes; Agents, also reachable through an A2A
v1.0 adapter; and third-party services hosted through the `ma2a-service` SDK. Every one of them
needs the caller's signed grant and the serving Endpoint's own exposure in one shared Space.

Start with [Quickstart](docs/quickstart.md), then review the
[architecture](docs/architecture.md), [security boundaries](docs/security.md), and
[limitations](docs/limitations.md).

- [Operations and troubleshooting](docs/operations.md)
- [Platform and ARM64 evidence](docs/platform-support.md)
- [Build and verification](docs/build.md)
- [Endpoint-centric CLI](docs/cli.md)
- [Storage and backup boundary](docs/storage.md)
- [Connectivity](docs/connectivity.md)
- [Relay reachability](docs/reachability.md)
- [Local Runtime API v1](docs/protocol/local-api-v1.md)
- [Echo Protocol v1](docs/protocol/echo-v1.md)
- [Services v1: Files, Transfer, Exec, Shell, Proxy](docs/protocol/services-v1.md)
- [Process v1](docs/protocol/process-v1.md)
- [Agent v1](docs/protocol/agent-v1.md) and the [A2A adapter](docs/protocol/a2a-adapter.md)
- [Hosted services v1](docs/protocol/service-v1.md)
- [Private IPC and daemon lifecycle](docs/private-ipc.md)
