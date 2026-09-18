import type { Tone } from "./viz/tone"

export type ConnectionState = "online" | "uncertain" | "offline"
export type EndpointStatus = "active" | "degraded" | "offline"
export type ObservedPath = "direct" | "relay" | "mixed" | "unknown"
export type SyncState = "current" | "catching-up" | "stalled" | "unknown"
export type RelayKind = "private" | "public"
export type RelayStatus = "eligible" | "disabled"

export type EndpointView = {
  readonly id: string
  readonly status: EndpointStatus
  readonly observedPath: ObservedPath
}

export type SpaceMemberView = {
  readonly endpointId: string
  readonly label: string
  readonly echo: boolean
  readonly relayProvider: boolean
}

export type SpaceView = {
  readonly id: string
  readonly name: string
  readonly memberCount: number
  readonly sync: SyncState
  readonly generation: number
  readonly chainHash: string
  readonly members: readonly SpaceMemberView[]
  readonly revokedCount: number
}

export type RelayView = {
  readonly endpointId: string
  readonly kind: RelayKind
  readonly status: RelayStatus
  readonly coveredSpaceIds: readonly string[]
}

export type ObservationPath = "connecting" | "direct" | "relay" | "mixed_or_unknown"

export type ObservationView = {
  readonly atMs: number
  readonly path: ObservationPath
  readonly rttMs: number | undefined
  readonly errorClass: string
}

export type PeerConnectionView = {
  readonly endpointId: string
  readonly state: "connecting" | "connected" | "failed"
  readonly path: "direct" | "relay" | "mixed or unknown"
  readonly rttMs: number | undefined
  readonly observations: readonly ObservationView[]
}

export type ControlRoundView = {
  readonly atMs: number
  readonly peerCount: number
  readonly outcome: "succeeded" | "failed" | "empty"
}

/** The four states named by docs/reachability.md, in the order they progress. */
export type ReachabilityStateName =
  | "NoActiveSpaces"
  | "DegradedNoCommonHome"
  | "AwaitingIrohHome"
  | "IrohHomeConnected"

export type ReachabilityView = {
  readonly state: ReachabilityStateName
  readonly tone: Tone
  readonly status: "reachable" | "degraded" | "unknown" | "unreachable"
  readonly path: string
  readonly detail: string
}

export type RuntimeViewData = {
  readonly revision: number
  readonly connection: ConnectionState
  readonly runtimeVersion: string
  readonly endpoint: EndpointView
  readonly spaces: readonly SpaceView[]
  readonly relays: readonly RelayView[]
  readonly observedRelayState: {
    readonly private_relay_online: boolean
    readonly public_relay_online: boolean
  }
  readonly reachability: ReachabilityView
  readonly controlSync: {
    readonly peer_endpoint_ids: readonly string[]
  }
  readonly peerConnections: readonly PeerConnectionView[]
  readonly controlRounds: readonly ControlRoundView[]
  readonly echoTotals: { readonly successes: number; readonly failures: number }
  readonly uiAuth: {
    readonly initialized: boolean
    readonly password_set: boolean
    readonly active_sessions: number
  }
}
