import type { EndpointId, SpaceId } from "./generated"

export type EndpointView = {
  readonly endpoint_id: EndpointId
  readonly runtime_version: string
  readonly online: boolean
}

export type SpaceView = {
  readonly space_id: SpaceId
  readonly name: string
  readonly member_count: number
}

export type ControlSyncView = {
  readonly peer_endpoint_ids: readonly EndpointId[]
  readonly synchronized: boolean
}

export type UiAuthView = {
  readonly initialized: boolean
  readonly password_set: boolean
  readonly active_sessions: number
}

export type CapabilityFlags = {
  readonly spaces: boolean
  readonly control_sync: boolean
  readonly private_relay: boolean
  readonly public_relay: boolean
  readonly echo: boolean
  readonly events: boolean
}

export type HandshakeView = {
  readonly runtime_version: string
  readonly endpoint_id: EndpointId
  readonly revision: number
  readonly initialized: boolean
  readonly password_set: boolean
  readonly capabilities: CapabilityFlags
}

export type RelayCandidateView = {
  readonly endpoint_id: EndpointId
  readonly relay_kind: string
  readonly eligible: boolean
}

export type ObservedRelayStateView = {
  readonly private_relay_online: boolean
  readonly public_relay_online: boolean
}

export type ReachabilityView = { readonly direct: boolean; readonly relayed: boolean }

export type EchoSummaryView = { readonly successes: number; readonly failures: number }

export type RuntimeSnapshot = {
  readonly revision: number
  readonly endpoint: EndpointView
  readonly spaces: readonly SpaceView[]
  readonly control_sync: ControlSyncView
  readonly relay_candidates: readonly RelayCandidateView[]
  readonly observed_relay_state: ObservedRelayStateView
  readonly reachability: ReachabilityView
  readonly recent_echo_summary: EchoSummaryView
  readonly ui_auth: UiAuthView
}

export type PrivateRelayView = {
  readonly configured: boolean
  readonly mode: "native_tls" | "external_termination"
  readonly host: string
  readonly port: number
  readonly online: boolean
}

export type PublicRelayView = {
  readonly configured: boolean
  readonly url: string | null
  readonly online: boolean
}

export type RuntimeStatusView = {
  readonly revision: number
  readonly initialized: boolean
  readonly shutting_down: boolean
}

export type EchoReplyView = {
  readonly target_endpoint_id: EndpointId
  readonly payload: string
}

export type CommandResult =
  | { readonly type: "handshake"; readonly payload: HandshakeView }
  | { readonly type: "status"; readonly payload: RuntimeStatusView }
  | { readonly type: "endpoint_info"; readonly payload: EndpointView }
  | { readonly type: "space_created"; readonly payload: SpaceView }
  | { readonly type: "spaces"; readonly payload: readonly SpaceView[] }
  | { readonly type: "space"; readonly payload: SpaceView }
  | { readonly type: "space_invitation_created"; readonly payload: SpaceView }
  | { readonly type: "space_redeemed"; readonly payload: SpaceView }
  | { readonly type: "space_revoked"; readonly payload: SpaceView }
  | { readonly type: "control_sync_status"; readonly payload: ControlSyncView }
  | { readonly type: "control_sync_triggered"; readonly payload: ControlSyncView }
  | { readonly type: "private_relay_configured"; readonly payload: PrivateRelayView }
  | { readonly type: "private_relay_status"; readonly payload: PrivateRelayView }
  | { readonly type: "public_relay_configured"; readonly payload: PublicRelayView }
  | { readonly type: "public_relay_status"; readonly payload: PublicRelayView }
  | { readonly type: "echo"; readonly payload: EchoReplyView }
  | { readonly type: "ui_password_set"; readonly payload: UiAuthView }
  | { readonly type: "ui_password_reset"; readonly payload: UiAuthView }
  | { readonly type: "sessions_revoked"; readonly payload: UiAuthView }
  | { readonly type: "snapshot"; readonly payload: RuntimeSnapshot }
  | { readonly type: "shutting_down"; readonly payload: Record<string, never> }
