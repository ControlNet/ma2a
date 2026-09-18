import { expect, test } from "vitest"

import { runtimeSnapshot } from "./api/test-fixtures"
import { runtimeViewFromSnapshot } from "./runtime-view"

test("projects only authoritative snapshot state into the console view", () => {
  const snapshot = {
    ...runtimeSnapshot(9),
    spaces: [
      {
        space_id: "ab".repeat(32),
        name: "Operations",
        member_count: 2,
        generation: 4,
        chain_hash: "dd".repeat(32),
        members: [
          {
            endpoint_id: "00".repeat(32),
            label: "operator",
            echo: true,
            relay_provider: false,
          },
          {
            endpoint_id: "cd".repeat(32),
            label: "peer",
            echo: true,
            relay_provider: false,
          },
        ],
        revoked_count: 1,
      },
    ],
    control_sync: { peer_endpoint_ids: ["cd".repeat(32)] },
    connections: [
      {
        endpoint_id: "cd".repeat(32),
        state: "connected" as const,
        path: "relay" as const,
        rtt_ms: 34,
        observations: [
          {
            observed_at_ms: 1_700_000_000_000,
            path: "relay" as const,
            rtt_ms: 34,
            error_class: "none" as const,
          },
        ],
      },
    ],
    private_relay_candidates: [
      {
        provider_endpoint_id: "ef".repeat(32),
        relay_url: "https://relay.example",
        covered_space_ids: ["ab".repeat(32)],
        home_compatible: true,
      },
    ],
    public_relay_fallbacks: [
      { relay_url: "https://public.example", enabled: true, observed_connected: false },
    ],
    control_rounds: [{ at_ms: 1_700_000_000_000, peer_count: 1, outcome: "succeeded" as const }],
    observed_relay_state: { private_relay_online: true, public_relay_online: false },
    reachability: { state: "IrohHomeConnected" as const, direct: false, relayed: true },
    recent_echo_summary: { successes: 3, failures: 1 },
    ui_auth: { initialized: true, password_set: true, active_sessions: 2 },
  }

  const view = runtimeViewFromSnapshot(snapshot, "online")

  expect(view).toMatchObject({
    revision: 9,
    connection: "online",
    endpoint: { observedPath: "relay", status: "active" },
    controlSync: { peer_endpoint_ids: ["cd".repeat(32)] },
    peerConnections: [
      { endpointId: "cd".repeat(32), state: "connected", path: "relay", rttMs: 34 },
    ],
    echoTotals: { successes: 3, failures: 1 },
    uiAuth: { active_sessions: 2 },
  })
  expect(view.spaces[0]).toMatchObject({ name: "Operations", memberCount: 2, generation: 4 })
  expect(view.privateRelayCandidates[0]).toMatchObject({
    providerEndpointId: "ef".repeat(32),
    relayUrl: "https://relay.example",
    homeCompatible: true,
  })
  expect(view.publicRelayFallbacks[0]).toMatchObject({
    relayUrl: "https://public.example",
    enabled: true,
    observedConnected: false,
  })
})

test("a public fallback carries no Endpoint identity and no Space coverage", () => {
  const snapshot = {
    ...runtimeSnapshot(3),
    public_relay_fallbacks: [
      { relay_url: "https://public.example", enabled: true, observed_connected: true },
    ],
  }

  const [fallback] = runtimeViewFromSnapshot(snapshot, "online").publicRelayFallbacks

  expect(fallback).toBeDefined()
  expect(Object.keys(fallback ?? {})).toEqual(["relayUrl", "enabled", "observedConnected"])
})

test("reachability is taken from the Runtime, never rebuilt from provider facts", () => {
  const snapshot = {
    ...runtimeSnapshot(4),
    reachability: { state: "DegradedNoCommonHome" as const, direct: false, relayed: false },
    observed_relay_state: { private_relay_online: true, public_relay_online: false },
  }

  const view = runtimeViewFromSnapshot(snapshot, "online")

  expect(view.reachability.state).toBe("DegradedNoCommonHome")
  expect(view.observedRelayState.privateRelayProviderRunning).toBe(true)
})
