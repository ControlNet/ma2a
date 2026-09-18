import type { RuntimeSnapshot } from "./api/client"
import type { ConnectionState, RuntimeViewData } from "./view-model"

/**
 * docs/reachability.md names four states. They are derived here from the only
 * fields the snapshot publishes, and never softened into a score.
 */
function reachabilityView(snapshot: RuntimeSnapshot): RuntimeViewData["reachability"] {
  if (snapshot.spaces.length === 0) {
    return {
      state: "NoActiveSpaces",
      tone: "none",
      status: "unknown",
      path: "No active Space",
      detail: "A zero-Space Endpoint contributes no private relay candidate at all.",
    }
  }
  const online =
    snapshot.observed_relay_state.private_relay_online ||
    snapshot.observed_relay_state.public_relay_online
  if (online) {
    return {
      state: "IrohHomeConnected",
      tone: "direct",
      status: "reachable",
      path: "Home relay connected",
      detail: "Iroh reports a connected home drawn from the candidate map MA2A supplied.",
    }
  }
  if (snapshot.relay_candidates.length > 0) {
    return {
      state: "AwaitingIrohHome",
      tone: "relay",
      status: "unknown",
      path: "Awaiting an Iroh home",
      detail: "Compatible candidates exist. Iroh has not reported a connected home yet.",
    }
  }
  return {
    state: "DegradedNoCommonHome",
    tone: "failed",
    status: "degraded",
    path: "No common home relay",
    detail:
      "No relay covers every active Space and public fallback is off. Direct paths may still work.",
  }
}

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
  const reachability = reachabilityView(snapshot)
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
      generation: space.generation,
      chainHash: space.chain_hash,
      members: space.members.map((member) => ({
        endpointId: member.endpoint_id,
        label: member.label,
        echo: member.echo,
        relayProvider: member.relay_provider,
      })),
      revokedCount: space.revoked_count,
    })),
    relays: snapshot.relay_candidates.map((relay) => ({
      endpointId: relay.endpoint_id,
      kind: relay.relay_kind === "public" ? "public" : "private",
      status: relay.eligible ? "eligible" : "disabled",
      coveredSpaceIds: relay.covered_space_ids,
    })),
    observedRelayState: snapshot.observed_relay_state,
    reachability,
    controlSync: snapshot.control_sync,
    peerConnections: snapshot.connections.map((peer) => ({
      endpointId: peer.endpoint_id,
      state: peer.state,
      path: peer.path === "mixed_or_unknown" ? "mixed or unknown" : peer.path,
      rttMs: peer.rtt_ms ?? undefined,
      observations: peer.observations.map((observation) => ({
        atMs: observation.observed_at_ms,
        path: observation.path,
        rttMs: observation.rtt_ms ?? undefined,
        errorClass: observation.error_class,
      })),
    })),
    controlRounds: snapshot.control_rounds.map((round) => ({
      atMs: round.at_ms,
      peerCount: round.peer_count,
      outcome: round.outcome,
    })),
    echoTotals: snapshot.recent_echo_summary,
    uiAuth: snapshot.ui_auth,
  }
}
