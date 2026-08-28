import type { RuntimeViewData } from "../view-model"

const ENDPOINT_ID = "test-endpoint-alpha-00000000000000000000000000000001"

export const EMPTY_RUNTIME_FIXTURE = {
  revision: 8,
  connection: "online",
  endpoint: {
    id: ENDPOINT_ID,
    status: "active",
    observedPath: "unknown",
  },
  spaces: [],
  relays: [],
  reachability: {
    status: "unknown",
    path: "No target selected",
    detail: "Run an Echo test to observe a target-specific path.",
  },
  echoHistory: [],
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
      id: "test-relay-local",
      url: "https://relay.test.invalid",
      kind: "private",
      compatibility: "home-compatible",
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
      id: "test-relay-space-only",
      url: "https://space-relay.test.invalid",
      kind: "private",
      compatibility: "space-only",
      status: "eligible",
    },
    {
      id: "test-relay-public",
      url: "https://public-relay.test.invalid",
      kind: "public",
      compatibility: "fallback",
      status: "disabled",
    },
  ],
  reachability: {
    status: "degraded",
    path: "Relay observed by Iroh",
    detail: "No Private Relay is compatible with every active Space.",
  },
  echoHistory: [
    {
      requestId: "test-request-01",
      target: "test-endpoint-bravo",
      status: "echoed",
      path: "relay",
    },
    {
      requestId: "test-request-02",
      target: "test-endpoint-charlie",
      status: "denied",
      path: "unknown",
    },
  ],
} satisfies RuntimeViewData
