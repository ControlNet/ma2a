export type ConnectionState = "online" | "uncertain" | "offline"
export type EndpointStatus = "active" | "degraded" | "offline"
export type ObservedPath = "direct" | "relay" | "mixed" | "unknown"
export type SyncState = "current" | "catching-up" | "stalled"
export type RelayKind = "private" | "public"
export type RelayStatus = "eligible" | "disabled"

export type EndpointView = {
  readonly id: string
  readonly status: EndpointStatus
  readonly observedPath: ObservedPath
}

export type SpaceView = {
  readonly id: string
  readonly name: string
  readonly memberCount: number
  readonly sync: SyncState
}

export type RelayView = {
  readonly endpointId: string
  readonly kind: RelayKind
  readonly status: RelayStatus
}

export type ReachabilityView = {
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
    readonly synchronized: boolean
  }
  readonly echoTotals: { readonly successes: number; readonly failures: number }
  readonly uiAuth: {
    readonly initialized: boolean
    readonly password_set: boolean
    readonly active_sessions: number
  }
}
