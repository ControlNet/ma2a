# MA2A Phase 2–5 Execution Roadmap

Repository: `ControlNet/ma2a`

Phase 1 correctness baseline:

`06a9bad5da23cefb9941a824aaa34ff21300327a`

Treat Phase 1 as frozen infrastructure.

The objective of Phases 2–5 is to evolve MA2A from:

```text
authenticated Endpoint connectivity
+ Spaces
+ routing
+ authorization
+ Echo
```

into:

```text
remote capability fabric
→ persistent workload fabric
→ Agent-to-Agent runtime
→ extensible Service platform
```

Execute Phases 2 through 5 in order.

Do not wait for human confirmation between phases unless a fundamental architectural contradiction is discovered.

Use atomic commits and establish a clean verification/freeze point at the end of every phase.

---

# Global invariants inherited from Phase 1

These are architectural constraints, not implementation suggestions.

## Identity

A Runtime owns exactly one persistent Endpoint identity.

Do not introduce:

```text
Process identity as cryptographic identity
Agent identity as cryptographic identity
Service identity as cryptographic identity
Device/User/Account hierarchy
```

Process, Agent, and Service objects are resources hosted by an Endpoint.

Remote transport authentication remains Endpoint-to-Endpoint.

---

## Space authorization

Normal operations remain endpoint-centric:

```text
caller Endpoint
→ target Endpoint
→ requested operation/resource
```

The caller does not select the authoritative Space.

The receiver searches its verified current Space state and must find:

> one complete Space authorization path that independently permits the requested operation.

Never compose partial permissions across Spaces.

Do not expose the selected authorizing Space to ordinary remote callers.

---

## Durable state

SQLite / Store is durable truth.

Actor/runtime state is applied projection.

Any durable desired state that manages a runtime resource must have explicit convergence semantics.

---

## Replay

RequestId remains a side-effect boundary.

For mutation operations:

```text
same RequestId + same canonical request
→ replay / terminal state
→ never duplicate side effect

same RequestId + different request
→ conflict
```

Use the Phase-1 committed / indeterminate mutation machinery rather than creating per-service replay systems.

---

## Phase boundaries

Do not prematurely implement later phases.

Especially:

```text
Phase 2:
no persistent Process objects
no Agent runtime
no generic plugin framework

Phase 3:
no Agent semantics
no distributed workflow engine

Phase 4:
no delegation/capability chains
no generic third-party service ecosystem
no multi-agent orchestration

Phase 5:
no advanced Space governance
no cross-Space implicit permission composition
no workflow scheduler
```

---

# Phase 2 — Remote Capability Layer

## Goal

Transform MA2A from an authenticated Echo network into a useful remote-computing fabric.

Implement five built-in capability families:

```text
Files
Transfer
Exec
Shell
Proxy
```

These are built-in, closed services.

Do not design the generic Service ecosystem yet.

The existing Phase-1 sealed Echo dispatch seam should evolve from:

```text
Echo only
```

to an exhaustive built-in set conceptually equivalent to:

```text
Echo
Files
Transfer
Exec
Shell
Proxy
```

The exact internal protocol decomposition and ALPN layout should be designed deliberately.

---

## 2.1 Built-in service foundation

Extend the Phase-1 service model so multiple explicitly known services can coexist.

Requirements:

- exhaustive typed service/operation discriminants;
- bounded canonical request metadata;
- transport-authenticated caller Endpoint;
- authorization before reading attacker-controlled large bodies;
- per-service concurrency limits;
- operation deadlines;
- cancellation;
- structured audit metadata;
- no payload/content logging;
- no runtime dynamic registration yet.

Define service-specific authorization using the existing Space model.

If additional Phase-2 capability bits/policies are required, add the smallest explicit model necessary.

Do not introduce generic Grants or Delegation.

---

# 2.2 Files service

Implement bounded remote filesystem operations.

Minimum operations:

```text
stat
list
mkdir
read-small
write-small
rename
remove
```

File paths must have deterministic remote semantics.

Do not expose ambiguous process-dependent path interpretation.

Use configured/exported roots or another explicit namespace model.

Define:

```text
path normalization
relative path semantics
symlink behavior
file/directory distinction
overwrite semantics
atomic replacement semantics
metadata returned
error mapping
```

Small reads/writes may use ordinary bounded messages.

Large file content must use Transfer rather than increasing ordinary RPC limits.

CLI examples may eventually resemble:

```text
ma2a file ls ENDPOINT ROOT:/path
ma2a file stat ENDPOINT ROOT:/path
ma2a file get ...
ma2a file put ...
```

Exact syntax may differ.

---

# 2.3 Transfer service

Implement streaming large-data transfer.

Required properties:

```text
bounded memory
backpressure
content length when known
stream cancellation
progress
integrity verification
temporary staging
atomic finalization
partial-transfer cleanup
```

Use a strong content hash such as BLAKE3 where appropriate.

Define resume semantics explicitly.

Do not silently claim resumability across daemon restart unless metadata required to prove it is persisted.

Investigate whether current `iroh-blobs` is a good implementation substrate now that MA2A has a mature authorization/isolation layer.

Do not adopt it merely because it exists.

Compare:

```text
native MA2A QUIC streaming
vs
iroh-blobs adapter
```

against:

```text
authorization integration
Endpoint targeting
bounded resources
resume support
dependency complexity
protocol control
```

Document the decision.

Transfer must support more than Files so future Process/Agent artifacts can reuse it.

---

# 2.4 Exec service

Implement non-interactive one-shot execution.

A request should conceptually support:

```text
program
argv
working directory
bounded environment additions
optional stdin
timeout
```

Response/stream:

```text
stdout
stderr
exit status
termination reason
duration
```

Do not make the primary API a shell command string.

Use typed argv semantics.

Exec is ephemeral:

```text
request starts operation
client can stream output
operation exits
resource disappears
```

It is not a persistent Process object.

Define behavior for:

```text
client cancellation
network loss
timeout
runtime shutdown
stdout/stderr backpressure
large output
```

Do not silently detach an Exec request.

Detached/durable execution belongs to Phase 3.

---

# 2.5 Shell service

Implement interactive terminal execution separately from Exec.

Required semantics:

```text
PTY/ConPTY creation
stdin
stdout/stderr/terminal stream
terminal resize
exit
cancel
half-close/disconnect behavior
```

Cross-platform support:

```text
Linux/macOS PTY
Windows ConPTY or appropriate native abstraction
```

Shell sessions are Phase-2 ephemeral resources.

They do not survive Runtime restart.

Do not turn Shell into Process persistence.

Use bounded stream queues and backpressure.

---

# 2.6 Proxy service

Implement Endpoint-targeted byte-stream proxying.

Phase-2 Proxy should provide an ephemeral stream such as:

```text
caller
→ MA2A target Endpoint
→ target-side TCP destination
```

Required semantics:

```text
TCP connect
bidirectional byte stream
half-close
backpressure
connection failure mapping
idle/operation timeout
cancellation
connection limits
```

Local port-forward CLI support may map accepted local connections to individual MA2A Proxy streams.

Do not implement durable remote listeners yet.

Persistent listener resources may be considered later.

---

# 2.7 Phase-2 API and UX

Expose the new capabilities through:

```text
Runtime API
CLI
Web visibility/status where useful
```

Do not force every streaming interaction through the snapshot API.

Separate:

```text
control/state API
```

from:

```text
high-volume operation streams
```

Use Phase-1 snapshot/event infrastructure only for state/projection information.

---

# Phase-2 acceptance gate

Before Phase 2 is frozen, real multi-Endpoint E2E must demonstrate:

```text
Files read/write/list
large transfer with integrity verification
transfer cancellation
one-shot Exec
interactive Shell resize/input/output
TCP Proxy byte-for-byte forwarding
```

Also verify:

```text
unrelated Endpoint denied
revoked Endpoint denied
one valid shared Space permits operation
two incomplete Spaces cannot combine permissions
zero-Space Endpoint exposes no Phase-2 services
resource limits remain bounded
Runtime restart leaves no falsely active ephemeral operation
```

Freeze Phase 2 before introducing persistent workloads.

---

# Phase 3 — Persistent Process Layer

## Goal

Introduce durable remote workload identity.

Phase 2 Exec answers:

> Run this command now.

Phase 3 Process answers:

> Create a workload that remains independently manageable after this client operation ends.

---

# 3.1 Process model

Introduce a logical identifier such as:

```text
ProcessId
```

This is a resource ID, not a cryptographic identity.

Define a durable Process specification and state machine.

Conceptually:

```text
Created
Starting
Running
Exited
Failed
Terminated
Lost
```

Avoid claiming a process is running solely because a PID exists.

---

# 3.2 Persistent Process creation

Provide a durable start operation.

Process specification should reuse Exec concepts:

```text
program
argv
cwd
environment
stdin policy
output policy
resource metadata
```

RequestId semantics must prevent duplicate Process creation.

The creation result must bind:

```text
RequestId
→ exactly one ProcessId
```

across retry/restart ambiguity.

---

# 3.3 Process management

Minimum operations:

```text
process start
process list
process inspect
process logs
process attach
process stdin
process signal
process terminate
process delete/archive
```

Define terminal-state retention policy.

---

# 3.4 Durable output

Process output cannot depend solely on a connected client.

Implement bounded durable or recoverable logs with explicit sequence/cursor semantics.

Requirements:

```text
stdout/stderr ordering semantics
bounded retention
cursor
attach from current position
read historical output
overflow behavior
```

Do not permit unbounded disk growth.

---

# 3.5 Runtime restart recovery

This is the core distinction between Phase 2 Exec and Phase 3 Process.

A Process should at minimum survive:

```text
client disconnect
network interruption
MA2A client restart
```

The Phase must explicitly determine and implement MA2A Runtime restart behavior.

Preferred target:

> A running managed Process remains manageable across MA2A daemon restart when the underlying OS process still exists.

Do not recover by trusting PID alone.

Use a process incarnation / ownership proof or supervisor architecture that prevents attaching to an unrelated PID after reuse.

Investigate whether an MA2A-owned lightweight process supervisor/sidecar is the cleanest cross-platform model.

Machine reboot semantics must be explicit.

If processes do not survive host reboot, reopen them as a proven terminal/lost state rather than pretending they remain running.

---

# 3.6 Process attach

Interactive attach should reuse Phase-2 streaming primitives where possible.

Avoid building a second unrelated stream implementation.

Process attach is different from Shell:

```text
Shell:
stream owns an ephemeral terminal

Process:
terminal/workload exists independently of this attachment
```

Multiple-reader or single-writer semantics must be explicit.

---

# Phase-3 acceptance gate

Demonstrate:

```text
start Process
disconnect client
Process continues

new client lists/inspects Process

read retained logs

attach to running Process

terminate Process

retry Process creation with same RequestId
→ no duplicate

daemon restart
→ running Process is rediscovered/reconciled safely
```

Include adversarial tests for:

```text
PID reuse
supervisor loss
daemon crash during Process creation
daemon crash after child creation but before response
log retention overflow
attach disconnect
```

Freeze Process lifecycle before adding Agent semantics.

---

# Phase 4 — Agent Runtime / A2A

## Goal

Introduce Agent as a first-class logical workload hosted by an Endpoint.

This is where MA2A becomes genuinely Agent-to-Agent.

An Agent is not a new network trust identity.

Conceptually:

```text
Endpoint
 ├── Process
 ├── Process
 └── Agent
```

Remote cryptographic authentication remains:

```text
Endpoint ↔ Endpoint
```

AgentId is application-level attribution/routing.

---

# 4.1 Agent model

Define minimal types such as:

```text
AgentId
AgentDefinition / Profile
AgentInstance
AgentTaskId
AgentTaskState
```

Avoid over-modeling Agent personality/model-provider concepts in Core.

Core should model lifecycle and protocol semantics, not individual LLM vendors.

---

# 4.2 Agent runtime

Build Agent execution on top of Phase-3 Process infrastructure where practical.

An Agent runtime should have:

```text
start
stop
inspect
task submit
task cancel
task status
stream output/events
```

The runtime adapter must define how MA2A talks to a local Agent implementation.

Prefer explicit typed adapter boundaries.

---

# 4.3 Agent task lifecycle

Introduce durable or clearly defined Agent task states.

Conceptually:

```text
Queued
Running
Waiting
Completed
Failed
Cancelled
```

Define:

```text
request replay
task creation idempotency
stream reconnect
result retention
cancellation
runtime restart
agent crash
```

---

# 4.4 Native Agent-to-Agent service

Create an MA2A-native Agent service for:

```text
task submission
messages
streamed task events
task status
task cancellation
artifact references
```

Remote request remains:

```text
caller Endpoint
→ target Endpoint
→ target AgentId
```

Authorization still resolves through one complete Space.

The logical AgentId must not replace Endpoint authentication.

---

# 4.5 Tool access

Agents should be able to use capabilities created in earlier phases:

```text
Files
Transfer
Exec
Shell where appropriate
Proxy
Process
```

Do not introduce Phase-6 delegation yet.

Until explicit delegated identity exists:

> remote authority derives from the hosting Endpoint.

Agent identity may be recorded for attribution but must not silently become an independent authorization principal.

---

# 4.6 Artifacts

Define a lightweight Agent artifact reference model.

Do not build a universal distributed artifact database.

Prefer reusing Transfer/content-addressed data primitives where possible.

Agent task results may reference:

```text
small inline result
or
artifact reference
```

Large payloads should not be embedded into Agent messages.

---

# 4.7 External A2A compatibility

Investigate the current external Agent-to-Agent ecosystem/protocol at implementation time.

Build compatibility as an adapter:

```text
external A2A
↕
MA2A Agent model
```

Do not let an external protocol redefine MA2A:

```text
Endpoint identity
Space authorization
routing
replay
resource lifecycle
```

Document unsupported semantic mismatches rather than weakening MA2A guarantees.

---

# Phase-4 acceptance gate

Demonstrate at least:

```text
Agent A hosted on Endpoint A
Agent B hosted on Endpoint B

A submits task to B

B streams progress
B uses local/remote MA2A tools
B produces result/artifact
A retrieves result

network interruption
→ reconnect/status recovery

same task-creation RequestId
→ no duplicate task

Agent process crash
→ correct terminal/recovery state

Runtime restart
→ Agent/Task state reconciles
```

Also prove logical Agent IDs cannot bypass Endpoint/Space authorization.

No multi-agent workflow scheduler yet.

---

# Phase 5 — Service Ecosystem

## Goal

Only now generalize the service architecture.

By Phase 5 MA2A should have real service families:

```text
Echo
Files
Transfer
Exec
Shell
Proxy
Process
Agent
```

Use these implementations as empirical evidence to discover the common abstraction.

Do not design the Service framework from Echo alone.

---

# 5.1 Extract common service semantics

Identify what the existing services genuinely share.

Likely dimensions include:

```text
service identity/type
protocol version
operations
request/response vs stream
resource limits
authorization requirements
concurrency policy
lifecycle
health
observability
cancellation
```

Do not force abstractions that only fit some services.

---

# 5.2 Service descriptor

Introduce a bounded typed descriptor representing remotely meaningful service capabilities.

Possible information:

```text
service type
protocol version(s)
operation set
transport mode
availability
bounded capability metadata
```

Descriptor metadata must not expose Space membership or authorization-path details.

---

# 5.3 Authorized service discovery

Allow an Endpoint to discover services available from another Endpoint.

This is **not** a global service registry.

Correct model:

```text
Endpoint A
→ Endpoint B
→ authorized service metadata
```

not:

```text
Universe
→ list every service everywhere
```

Service discovery must respect existing Endpoint/Space visibility semantics.

---

# 5.4 General service dispatch

Refactor built-in service dispatch into a coherent common framework without changing their established external semantics unnecessarily.

Prove the abstraction by migrating existing built-ins.

The framework must support at least:

```text
unary request/response
structured streaming
raw byte streaming
long-lived resource operations
```

without pretending those are identical.

---

# 5.5 Local service registration

Design the first real service hosting interface.

Avoid unstable in-process Rust ABI plugin loading.

Strongly consider an out-of-process local service host protocol:

```text
third-party service process
↔ local MA2A Runtime IPC
↔ authenticated remote MA2A peer
```

The Runtime should remain responsible for:

```text
network identity
remote authentication
Space authorization
resource admission
protocol framing
```

A third-party service must not get raw unrestricted control over MA2A network admission.

---

# 5.6 Service lifecycle

Support:

```text
register
start
ready
unavailable
stop
unregister
```

Define crash behavior and restart reconciliation.

A registered service that is currently dead must not be advertised as healthy.

---

# 5.7 Service SDK

After the local service contract stabilizes, provide an SDK.

Start with the language/runtime that minimizes architectural risk.

Rust-first is acceptable.

A language-neutral protocol can follow if the service-host boundary is clean.

Do not create code generation/schema machinery unless real services demonstrate the need.

---

# 5.8 Versioning

Phase 1 used exact protocol matches.

Phase 5 should define intentional Service version semantics.

Possible model:

```text
ServiceType
+
supported protocol versions
```

Do not silently negotiate arbitrary incompatible versions.

Make version selection deterministic and observable.

---

# Phase-5 acceptance gate

The Service ecosystem is complete only if a third-party example service can:

```text
register locally
become remotely discoverable when appropriate
receive authenticated/authorized calls
stream where required
handle cancellation
survive Runtime/service restart semantics
unregister cleanly
```

and existing built-ins continue to work through the common framework.

Demonstrate:

```text
built-in service
third-party example service
different protocol versions
service unavailable/restart
authorization denial
resource saturation
remote discovery
```

without introducing a global registry.

---

# Phase 2–5 cross-phase engineering requirements

## Tests

Every Phase must add:

```text
unit tests
Store tests where durable state exists
Runtime state-machine tests
real transport tests
multi-process tests
cross-platform compile/build checks
adversarial E2E
```

Do not reduce Phase-1 verification.

Phase-1 scenarios remain regression tests.

---

## Failure injection

Introduce deterministic fault points for new durable state machines.

Especially around:

```text
commit before response
resource created before Store commit
Store commit before resource application
network response loss
Runtime restart
child/service crash
stream cancellation
```

Do not rely on random timing to test these windows.

---

## Documentation

Create authoritative plans/docs for each phase.

Suggested files:

```text
.omo/plans/ma2a-phase-2.md
.omo/plans/ma2a-phase-3.md
.omo/plans/ma2a-phase-4.md
.omo/plans/ma2a-phase-5.md
```

Protocol documentation should be created alongside implementation.

---

# Suggested high-level dependency graph

```text
Phase 1
│
├─ Endpoint identity
├─ Space
├─ authorization
├─ routing
├─ relay
├─ Runtime
├─ replay
└─ Echo
        │
        ▼
Phase 2
Remote Capability Layer
│
├─ Files
├─ Transfer
├─ Exec
├─ Shell
└─ Proxy
        │
        ▼
Phase 3
Persistent Process Layer
│
├─ ProcessId
├─ lifecycle
├─ durable logs
├─ attach
└─ restart reconciliation
        │
        ▼
Phase 4
Agent Layer
│
├─ AgentId
├─ runtime
├─ tasks
├─ native A2A
├─ tools
└─ artifacts
        │
        ▼
Phase 5
Service Platform
│
├─ ServiceDescriptor
├─ discovery
├─ common dispatch
├─ local registration
├─ lifecycle
├─ versioning
└─ SDK
```

---

# Explicitly deferred beyond Phase 5

Do not implement these during this roadmap:

```text
advanced Grants
delegation chains
Agent-as-cryptographic-principal
authority transfer
multi-admin Space governance
cross-Space implicit permission composition
fleet/reusable enrollment
generic distributed workflow scheduler
multi-agent DAG orchestration
automatic peer/Agent scheduling
global service registry
distributed consensus
general CRDT control database
automatic relay-provider election
```

These belong to a future architecture pass after Phase 5 provides real usage evidence.

---

# Execution strategy

Execute Phase 2 → 3 → 4 → 5 continuously.

At the end of each phase:

1. perform a fresh architecture/correctness audit;
2. run the complete regression suite;
3. run the phase-specific repeated E2E;
4. resolve correctness blockers;
5. create a documented freeze commit;
6. continue to the next phase.

Do not stop for cosmetic or low-priority hardening unless it blocks the next phase.

Do not carry known state-machine correctness defects into the next phase.

---

# Final Phase-5 milestone

At the end of this roadmap, MA2A should support the following end-to-end scenario:

```text
Endpoint A
   │
   │ authenticated MA2A
   ▼
Endpoint B
   │
   ├─ Files
   ├─ Transfer
   ├─ Exec
   ├─ Shell
   ├─ Proxy
   │
   ├─ Persistent Process P
   │
   └─ Agent B
          │
          ├─ executes persistent/local work
          ├─ consumes MA2A services
          ├─ streams task state
          └─ returns artifacts

Agent A @ Endpoint A
          │
          └──── Agent-to-Agent task ────► Agent B
```

while preserving:

```text
Endpoint identity
Space isolation
single-complete-Space authorization
durable replay
Store/Runtime consistency
bounded resources
restart recovery
```

and with the built-in functionality now expressed through a mature Service framework suitable for third-party extension.

That state is the Phase-5 freeze milestone.

Only after reaching it should MA2A revisit advanced trust/delegation and distributed multi-Agent orchestration.
