import type { RuntimeSnapshot } from "./api/client"
import type { SpaceDetails } from "./api/codec"
import type { ConnectionState, RuntimeViewData } from "./view-model"
import type { Tone } from "./viz/tone"

const REACHABILITY_DETAIL: Record<RuntimeViewData["reachability"]["state"], string> = {
  NoActiveSpaces: "A zero-Space Endpoint contributes no private relay candidate at all.",
  DegradedNoCommonHome:
    "No relay covers every active Space and public fallback is off. Direct paths may still work.",
  AwaitingIrohHome: "Compatible candidates exist. Iroh has not reported a connected home yet.",
  IrohHomeConnected: "Iroh reports a connected home drawn from the candidate map MA2A supplied.",
}

const REACHABILITY_TONE: Record<RuntimeViewData["reachability"]["state"], Tone> = {
  NoActiveSpaces: "none",
  DegradedNoCommonHome: "failed",
  AwaitingIrohHome: "relay",
  IrohHomeConnected: "direct",
}

/**
 * The Runtime owns this state. It is projected verbatim and never reconstructed
 * from provider facts such as whether a local Private Relay Provider is running.
 */
function reachabilityView(snapshot: RuntimeSnapshot): RuntimeViewData["reachability"] {
  const state = snapshot.reachability.state
  return { state, tone: REACHABILITY_TONE[state], detail: REACHABILITY_DETAIL[state] }
}

export function runtimeViewFromSnapshot(
  snapshot: RuntimeSnapshot,
  connection: ConnectionState,
  details: ReadonlyMap<string, SpaceDetails> = new Map(),
  failedDetails: ReadonlySet<string> = new Set(),
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
      generation: space.generation,
      chainHash: space.chain_hash,
      membersError: failedDetails.has(space.space_id),
      members: (details.get(space.space_id)?.space.chain_hash === space.chain_hash
        ? details.get(space.space_id)?.members
        : undefined
      )?.map((member) => ({
        endpointId: member.endpoint_id,
        label: member.label,
        echo: member.echo,
        relayProvider: member.relay_provider,
      })),
      revokedCount: space.revoked_count,
    })),
    privateRelayCandidates: snapshot.private_relay_candidates.map((relay) => ({
      providerEndpointId: relay.provider_endpoint_id,
      relayUrl: relay.relay_url,
      coveredSpaceIds: relay.covered_space_ids,
      homeCompatible: relay.home_compatible,
    })),
    publicRelayFallbacks: snapshot.public_relay_fallbacks.map((fallback) => ({
      relayUrl: fallback.relay_url,
      enabled: fallback.enabled,
      observedConnected: fallback.observed_connected,
    })),
    observedRelayState: {
      privateRelayProviderRunning: snapshot.observed_relay_state.private_relay_provider_running,
      publicRelayConnected: snapshot.observed_relay_state.public_relay_connected,
    },
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
