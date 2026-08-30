import { expect, test } from "vitest"

import { runtimeSnapshot } from "./api/test-fixtures"
import { runtimeViewFromSnapshot } from "./runtime-view"

test("projects only authoritative snapshot state into the console view", () => {
  const snapshot = {
    ...runtimeSnapshot(9),
    spaces: [{ space_id: "ab".repeat(32), name: "Operations", member_count: 2 }],
    control_sync: { peer_endpoint_ids: ["cd".repeat(32)], synchronized: false },
    relay_candidates: [{ endpoint_id: "ef".repeat(32), relay_kind: "private", eligible: true }],
    observed_relay_state: { private_relay_online: true, public_relay_online: false },
    reachability: { direct: false, relayed: true },
    recent_echo_summary: { successes: 3, failures: 1 },
    ui_auth: { initialized: true, password_set: true, active_sessions: 2 },
  }

  const view = runtimeViewFromSnapshot(snapshot, "online")

  expect(view).toMatchObject({
    revision: 9,
    connection: "online",
    endpoint: { observedPath: "relay", status: "active" },
    controlSync: { peer_endpoint_ids: ["cd".repeat(32)], synchronized: false },
    echoTotals: { successes: 3, failures: 1 },
    uiAuth: { active_sessions: 2 },
  })
  expect(view.spaces[0]).toMatchObject({ name: "Operations", memberCount: 2, sync: "catching-up" })
  expect(view.relays[0]).toMatchObject({
    endpointId: "ef".repeat(32),
    kind: "private",
    status: "eligible",
  })
})
