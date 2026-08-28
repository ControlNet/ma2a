export type ConnectionState = "online" | "uncertain" | "offline"
export type EndpointStatus = "active" | "degraded" | "offline"
export type ObservedPath = "direct" | "relay" | "mixed" | "unknown"
export type SyncState = "current" | "catching-up" | "stalled"
export type RelayKind = "private" | "public"
export type RelayCompatibility = "home-compatible" | "space-only" | "fallback"
export type RelayStatus = "eligible" | "selected" | "disabled" | "expired"
export type EchoStatus = "echoed" | "denied" | "failed"

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
  readonly id: string
  readonly url: string
  readonly kind: RelayKind
  readonly compatibility: RelayCompatibility
  readonly status: RelayStatus
}

export type ReachabilityView = {
  readonly status: "reachable" | "degraded" | "unknown" | "unreachable"
  readonly path: string
  readonly detail: string
}

export type EchoSummaryView = {
  readonly requestId: string
  readonly target: string
  readonly status: EchoStatus
  readonly path: ObservedPath
}

export type RuntimeViewData = {
  readonly revision: number
  readonly connection: ConnectionState
  readonly endpoint: EndpointView
  readonly spaces: readonly SpaceView[]
  readonly relays: readonly RelayView[]
  readonly reachability: ReachabilityView
  readonly echoHistory: readonly EchoSummaryView[]
}
