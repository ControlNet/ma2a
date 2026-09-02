import type { RuntimeSnapshot } from "./api/client"
import type { ConnectionState, RuntimeViewData } from "./view-model"

export function runtimeViewFromSnapshot(
  snapshot: RuntimeSnapshot,
  connection: ConnectionState,
): RuntimeViewData {
  const observedPath = snapshot.reachability.direct
    ? snapshot.reachability.relayed
      ? "mixed"
      : "direct"
    : snapshot.reachability.relayed
      ? "relay"
      : "unknown"
  const reachability: RuntimeViewData["reachability"] = snapshot.reachability.direct
    ? {
        status: "reachable",
        path: "Direct path observed",
        detail: "Iroh reports direct reachability.",
      }
    : snapshot.reachability.relayed
      ? {
          status: "reachable",
          path: "Relay path observed by Iroh",
          detail: "Iroh reports relayed reachability; MA2A does not select the home relay.",
        }
      : snapshot.relay_candidates.length === 0
        ? {
            status: "degraded",
            path: "DegradedNoCommonHome",
            detail:
              "No compatible relay candidate is available. Configure a common Private Relay or enable a Public fallback.",
          }
        : {
            status: "unknown",
            path: "No path observed",
            detail: "Compatible candidates exist, but Iroh has not reported an effective path.",
          }
  return {
    revision: snapshot.revision,
    connection,
    runtimeVersion: snapshot.endpoint.runtime_version,
    endpoint: {
      id: snapshot.endpoint.endpoint_id,
      status: snapshot.endpoint.online ? "active" : "offline",
      observedPath,
    },
    spaces: snapshot.spaces.map((space) => ({
      id: space.space_id,
      name: space.name,
      memberCount: space.member_count,
      sync: "unknown",
    })),
    relays: snapshot.relay_candidates.map((relay) => ({
      endpointId: relay.endpoint_id,
      kind: relay.relay_kind === "public" ? "public" : "private",
      status: relay.eligible ? "eligible" : "disabled",
    })),
    observedRelayState: snapshot.observed_relay_state,
    reachability,
    controlSync: snapshot.control_sync,
    peerConnections: snapshot.connections.map((peer) => ({
      endpointId: peer.endpoint_id,
      state: peer.state,
      path: peer.path === "mixed_or_unknown" ? "mixed or unknown" : peer.path,
      rttMs: peer.rtt_ms ?? undefined,
    })),
    echoTotals: snapshot.recent_echo_summary,
    uiAuth: snapshot.ui_auth,
  }
}
