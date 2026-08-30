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
    status: "unknown",
    path: "No target selected",
    detail: "Run an Echo test to observe a target-specific path.",
  },
  controlSync: { peer_endpoint_ids: [], synchronized: true },
  echoTotals: { successes: 0, failures: 0 },
  uiAuth: { initialized: true, password_set: true, active_sessions: 1 },
} satisfies RuntimeViewData

export const ONE_RUNTIME_FIXTURE = {
  ...EMPTY_RUNTIME_FIXTURE,
  revision: 12,
  spaces: [
    {
      id: "test-space-operations",
      name: "Operations",
      memberCount: 1,
      sync: "current",
    },
  ],
  relays: [
    {
      endpointId: "11".repeat(32),
      kind: "private",
      status: "eligible",
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
    },
    {
      id: "test-space-field",
      name: "Field",
      memberCount: 2,
      sync: "current",
    },
  ],
  relays: [
    ...ONE_RUNTIME_FIXTURE.relays,
    {
      endpointId: "22".repeat(32),
      kind: "private",
      status: "eligible",
    },
    {
      endpointId: "33".repeat(32),
      kind: "public",
      status: "disabled",
    },
  ],
  reachability: {
    status: "degraded",
    path: "Relay observed by Iroh",
    detail: "No Private Relay is compatible with every active Space.",
  },
  observedRelayState: { private_relay_online: true, public_relay_online: false },
  controlSync: { peer_endpoint_ids: ["44".repeat(32)], synchronized: false },
  echoTotals: { successes: 1, failures: 1 },
} satisfies RuntimeViewData
