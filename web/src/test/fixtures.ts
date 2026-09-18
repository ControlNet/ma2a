import type { RuntimeViewData } from "../view-model"

const ENDPOINT_ID = "test-endpoint-alpha-00000000000000000000000000000001"

export const EMPTY_RUNTIME_FIXTURE = {
  revision: 8,
  connection: "online",
  runtimeVersion: "0.1.0",
  endpoint: {
    id: ENDPOINT_ID,
    status: "active",
    observedPath: "unknown",
  },
  spaces: [],
  relays: [],
  observedRelayState: { private_relay_online: false, public_relay_online: false },
  reachability: {
    state: "NoActiveSpaces",
    tone: "none",
    status: "unknown",
    path: "No active Space",
    detail: "A zero-Space Endpoint contributes no private relay candidate at all.",
  },
  controlSync: { peer_endpoint_ids: [] },
  controlRounds: [],
  peerConnections: [],
  echoTotals: { successes: 0, failures: 0 },
  uiAuth: { initialized: true, password_set: true, active_sessions: 1 },
} satisfies RuntimeViewData

export const ONE_RUNTIME_FIXTURE = {
  ...EMPTY_RUNTIME_FIXTURE,
  revision: 12,
  reachability: {
    state: "AwaitingIrohHome",
    tone: "relay",
    status: "unknown",
    path: "Awaiting an Iroh home",
    detail: "Compatible candidates exist. Iroh has not reported a connected home yet.",
  },
  spaces: [
    {
      id: "test-space-operations",
      name: "Operations",
      memberCount: 1,
      sync: "current",
      generation: 0,
      chainHash: "aa".repeat(32),
      members: [{ endpointId: ENDPOINT_ID, label: "operator", echo: true, relayProvider: true }],
      revokedCount: 0,
    },
  ],
  relays: [
    {
      endpointId: "11".repeat(32),
      kind: "private",
      status: "eligible",
      coveredSpaceIds: ["test-space-operations"],
    },
  ],
} satisfies RuntimeViewData

export const MANY_RUNTIME_FIXTURE = {
  ...ONE_RUNTIME_FIXTURE,
  revision: 24,
  endpoint: {
    id: ENDPOINT_ID,
    status: "degraded",
    observedPath: "relay",
  },
  spaces: [
    ...ONE_RUNTIME_FIXTURE.spaces,
    {
      id: "test-space-laboratory",
      name: "Laboratory",
      memberCount: 3,
      sync: "catching-up",
      generation: 11,
      chainHash: "bb".repeat(32),
      members: [
        { endpointId: ENDPOINT_ID, label: "operator", echo: true, relayProvider: true },
        { endpointId: "44".repeat(32), label: "field-station-2", echo: true, relayProvider: false },
        { endpointId: "55".repeat(32), label: "lab-archive", echo: false, relayProvider: false },
      ],
      revokedCount: 1,
    },
    {
      id: "test-space-field",
      name: "Field",
      memberCount: 2,
      sync: "current",
      generation: 2,
      chainHash: "cc".repeat(32),
      members: [
        { endpointId: ENDPOINT_ID, label: "operator", echo: true, relayProvider: true },
        { endpointId: "44".repeat(32), label: "field-station-2", echo: true, relayProvider: false },
      ],
      revokedCount: 0,
    },
  ],
  relays: [
    ...ONE_RUNTIME_FIXTURE.relays,
    {
      endpointId: "22".repeat(32),
      kind: "private",
      status: "eligible",
      coveredSpaceIds: ["test-space-operations", "test-space-laboratory", "test-space-field"],
    },
    {
      endpointId: "33".repeat(32),
      kind: "public",
      status: "disabled",
      coveredSpaceIds: [],
    },
  ],
  reachability: {
    state: "DegradedNoCommonHome",
    tone: "failed",
    status: "degraded",
    path: "No common home relay",
    detail: "No Private Relay is compatible with every active Space.",
  },
  observedRelayState: { private_relay_online: true, public_relay_online: false },
  controlSync: { peer_endpoint_ids: ["44".repeat(32)] },
  peerConnections: [
    {
      endpointId: "44".repeat(32),
      state: "connected",
      path: "relay",
      rttMs: 34,
      observations: [
        { atMs: 1_700_000_000_000, path: "connecting", rttMs: undefined, errorClass: "none" },
        { atMs: 1_700_000_001_000, path: "relay", rttMs: 92, errorClass: "none" },
        { atMs: 1_700_000_002_000, path: "relay", rttMs: 34, errorClass: "none" },
      ],
    },
  ],
  controlRounds: [
    { atMs: 1_700_000_000_000, peerCount: 0, outcome: "empty" },
    { atMs: 1_700_000_070_000, peerCount: 1, outcome: "succeeded" },
    { atMs: 1_700_000_145_000, peerCount: 0, outcome: "failed" },
  ],
  echoTotals: { successes: 1, failures: 1 },
} satisfies RuntimeViewData
