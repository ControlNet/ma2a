import type { RuntimeViewData } from "../view-model"
import type { Lane, ObservedPeer } from "../viz/lane-map"
import type { Meter } from "../viz/meter-list"
import type { StackPart } from "../viz/stack-bar"
import type { ReachabilityState } from "../viz/state-machine"
import type { Tone } from "../viz/tone"

/** Documented Phase 1 bounds, from docs/connectivity.md and docs/protocol. */
export const LIMITS = {
  echoDeadlineMs: 10_000,
  connections: 128,
  members: 64,
  echoPayload: 4096,
  echoStreamsPerPeer: 16,
  spaces: 64,
} as const

export function shortId(value: string): string {
  return value.length <= 13 ? value : `${value.slice(0, 6)}…${value.slice(-4)}`
}

export function laneList(runtime: RuntimeViewData): Lane[] {
  return runtime.spaces.map((space) => ({
    spaceId: shortId(space.id),
    name: space.name,
    memberCount: space.memberCount,
  }))
}

function pathTone(peer: RuntimeViewData["peerConnections"][number]): Tone {
  if (peer.state === "failed") return "failed"
  if (peer.state === "connecting") return "none"
  if (peer.path === "direct") return "direct"
  if (peer.path === "relay") return "relay"
  return "none"
}

function badge(peer: RuntimeViewData["peerConnections"][number]): string {
  if (peer.state === "failed") return "last attempt failed"
  if (peer.state === "connecting") return "connecting"
  const rtt = peer.rttMs === undefined ? "rtt not observed" : `${peer.rttMs} ms`
  return `${peer.path} · ${rtt}`
}

/**
 * Every peer this Endpoint is authorized to reach, built from the signed member
 * sets of all active Spaces, then overlaid with whatever Iroh happens to have
 * observed. A member with no observation stays listed: membership is a signed
 * fact and does not depend on reachability.
 */
export function peerList(runtime: RuntimeViewData): ObservedPeer[] {
  const observed = new Map(runtime.peerConnections.map((peer) => [peer.endpointId, peer]))
  const members = new Map<string, string>()
  for (const space of runtime.spaces) {
    for (const member of space.members ?? []) {
      if (member.endpointId !== runtime.endpoint.id) members.set(member.endpointId, member.label)
    }
  }
  for (const id of runtime.controlSync.peer_endpoint_ids) {
    if (id !== runtime.endpoint.id && !members.has(id)) members.set(id, "peer")
  }
  for (const id of observed.keys()) {
    if (id !== runtime.endpoint.id && !members.has(id)) members.set(id, "peer")
  }
  return Array.from(members, ([id, label]) => {
    const peer = observed.get(id)
    if (peer === undefined) {
      return {
        id,
        shortId: shortId(id),
        state: label,
        tone: "none" as Tone,
        badge: "no transport observation",
      }
    }
    return { id, shortId: shortId(id), state: label, tone: pathTone(peer), badge: badge(peer) }
  })
}

export function observationTrack(
  runtime: RuntimeViewData,
  endpointId: string,
): {
  readonly atMs: number
  readonly tone: Tone
  readonly label: string
  readonly rttMs: number | undefined
}[] {
  const peer = runtime.peerConnections.find((candidate) => candidate.endpointId === endpointId)
  return (peer?.observations ?? []).map((observation) => ({
    atMs: observation.atMs,
    tone:
      observation.errorClass !== "none"
        ? "failed"
        : observation.path === "direct"
          ? "direct"
          : observation.path === "relay"
            ? "relay"
            : "none",
    label: observation.errorClass === "none" ? observation.path : observation.errorClass,
    rttMs: observation.rttMs,
  }))
}

export function pathParts(runtime: RuntimeViewData): StackPart[] {
  const peers = peerList(runtime)
  const count = (tone: Tone): number => peers.filter((peer) => peer.tone === tone).length
  return [
    { label: "direct", value: count("direct"), tone: "direct" },
    { label: "relay", value: count("relay"), tone: "relay" },
    { label: "failed", value: count("failed"), tone: "failed" },
    { label: "unobserved", value: count("none"), tone: "none" },
  ]
}

/** Everything MA2A is willing to hand Iroh, from both relay ontologies. */
export function candidateCount(runtime: RuntimeViewData): number {
  return (
    runtime.privateRelayCandidates.filter((relay) => relay.homeCompatible).length +
    runtime.publicRelayFallbacks.filter((fallback) => fallback.enabled).length
  )
}

export function boundMeters(runtime: RuntimeViewData): Meter[] {
  return [
    {
      label: "Owned connections",
      fraction: runtime.peerConnections.length / LIMITS.connections,
      value: `${runtime.peerConnections.length} / ${LIMITS.connections}`,
      tone: "accent",
    },
    {
      label: "Spaces",
      fraction: runtime.spaces.length / LIMITS.spaces,
      value: `${runtime.spaces.length} / ${LIMITS.spaces}`,
      tone: "accent",
      note: "Each Space authorizes on its own and is never combined with another.",
    },
    {
      label: "Candidates supplied to Iroh",
      fraction: candidateCount(runtime) === 0 ? 0 : 1,
      value: String(candidateCount(runtime)),
      tone: candidateCount(runtime) === 0 ? "none" : "direct",
      note: "Home-compatible Private Relays plus enabled public fallbacks. A listed candidate is not a reachability guarantee.",
    },
  ]
}

export const REACHABILITY_STATES: readonly ReachabilityState[] = [
  {
    name: "NoActiveSpaces",
    gloss: "No Space, so this Endpoint contributes no private candidate at all.",
    tone: "none",
  },
  {
    name: "DegradedNoCommonHome",
    gloss: "No relay covers every active Space and public fallback is off.",
    tone: "failed",
  },
  {
    name: "AwaitingIrohHome",
    gloss: "Compatible candidates exist. Iroh has not reported a connected home yet.",
    tone: "relay",
  },
  {
    name: "IrohHomeConnected",
    gloss: "Iroh reports a connected home drawn from the supplied map.",
    tone: "direct",
  },
]

export function echoSegments(runtime: RuntimeViewData): {
  readonly segments: readonly {
    readonly tone: Tone
    readonly value: number
    readonly label: string
  }[]
  readonly total: number
} {
  const { successes, failures } = runtime.echoTotals
  return {
    segments: [
      { tone: "direct", value: successes, label: "ok" },
      { tone: "failed", value: failures, label: "failed" },
    ],
    total: successes + failures,
  }
}
