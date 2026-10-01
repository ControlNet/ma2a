import type { EndpointId, SpaceId } from "./generated"
import type {
  AgentDefinedView,
  AgentEventPageView,
  AgentPageView,
  AgentSubmittedView,
  AgentTaskDetailsView,
  AgentTaskPageView,
  AgentTaskView,
  AgentUndefinedView,
  AgentView,
} from "./generated-agents"
import type { ProcessDetailsView, ProcessPageView, ProcessSummaryView } from "./generated-processes"
import type { OperationsView, ServiceExposureView } from "./generated-services"

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

export type SpaceIdentityView = {
  readonly space_id: SpaceId
  readonly name: string
}

export type ControlSyncView = {
  readonly peer_endpoint_ids: readonly EndpointId[]
}

export type SpaceMemberView = {
  readonly endpoint_id: EndpointId
  readonly label: string
  readonly echo: boolean
  readonly relay_provider: boolean
  readonly files_read: boolean
  readonly files_write: boolean
  readonly exec: boolean
  readonly shell: boolean
  readonly proxy: boolean
  readonly process: boolean
  readonly agent: boolean
}

export type SnapshotSpaceView = {
  readonly space_id: SpaceId
  readonly name: string
  readonly member_count: number
  readonly generation: number
  readonly chain_hash: string
  readonly revoked_count: number
}

export type SpaceDetailsView = {
  readonly revision: number
  readonly space: SnapshotSpaceView
  readonly members: readonly SpaceMemberView[]
}

export type SnapshotStampView = {
  readonly revision: number
  readonly runtime_boot_id: string
}

export type ConnectionObservationView = {
  readonly observed_at_ms: number
  readonly path: "connecting" | "direct" | "relay" | "mixed_or_unknown"
  readonly rtt_ms: number | null
  readonly error_class:
    | "none"
    | "transient"
    | "authorization"
    | "version"
    | "revocation"
    | "malformed_input"
    | "policy"
    | "cancelled"
}

export type ConnectionView = {
  readonly endpoint_id: EndpointId
  readonly state: "connecting" | "connected" | "failed"
  readonly path: "direct" | "relay" | "mixed_or_unknown"
  readonly rtt_ms: number | null
  readonly observations: readonly ConnectionObservationView[]
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

export type PrivateRelayCandidateView = {
  readonly provider_endpoint_id: EndpointId
  readonly relay_url: string
  readonly covered_space_ids: readonly SpaceId[]
  readonly home_compatible: boolean
}

/** External transport infrastructure: no Endpoint identity, no Space coverage. */
export type PublicRelayFallbackView = {
  readonly relay_url: string
  readonly enabled: boolean
  readonly observed_connected: boolean
}

export type ControlRoundView = {
  readonly at_ms: number
  readonly peer_count: number
  readonly outcome: "succeeded" | "failed" | "empty"
}

export type ObservedRelayStateView = {
  readonly private_relay_provider_running: boolean
  readonly public_relay_connected: boolean
}

export type ReachabilityStateName =
  | "NoActiveSpaces"
  | "DegradedNoCommonHome"
  | "AwaitingIrohHome"
  | "IrohHomeConnected"

export type ReachabilityView = {
  readonly state: ReachabilityStateName
  readonly direct: boolean
  readonly relayed: boolean
}

export type EchoSummaryView = { readonly successes: number; readonly failures: number }

export type RuntimeSnapshot = {
  readonly revision: number
  readonly endpoint: EndpointView
  readonly spaces: readonly SnapshotSpaceView[]
  readonly control_sync: ControlSyncView
  readonly connections: readonly ConnectionView[]
  readonly private_relay_candidates: readonly PrivateRelayCandidateView[]
  readonly public_relay_fallbacks: readonly PublicRelayFallbackView[]
  readonly control_rounds: readonly ControlRoundView[]
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
  readonly duration_ms: number
}

export type UiStatusView = {
  readonly running: boolean
  readonly url: string | null
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
  | { readonly type: "space_left"; readonly payload: SpaceIdentityView }
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
  | { readonly type: "ui_initialized"; readonly payload: UiAuthView }
  | { readonly type: "ui_status"; readonly payload: UiStatusView }
  | { readonly type: "space_details"; readonly payload: SpaceDetailsView }
  | { readonly type: "snapshot_stamp"; readonly payload: SnapshotStampView }
  | { readonly type: "snapshot"; readonly payload: RuntimeSnapshot }
  | { readonly type: "shutting_down"; readonly payload: Record<string, never> }
  | { readonly type: "space_granted"; readonly payload: SpaceMemberView }
  | { readonly type: "service_exposed"; readonly payload: ServiceExposureView }
  | { readonly type: "service_unexposed"; readonly payload: ServiceExposureView }
  | { readonly type: "service_exposures"; readonly payload: readonly ServiceExposureView[] }
  | { readonly type: "operations"; readonly payload: OperationsView }
  | { readonly type: "stream_opened"; readonly payload: Record<string, never> }
  | { readonly type: "processes"; readonly payload: ProcessPageView }
  | { readonly type: "process"; readonly payload: ProcessDetailsView }
  | { readonly type: "process_terminated"; readonly payload: ProcessSummaryView }
  | { readonly type: "process_deleted"; readonly payload: ProcessSummaryView }
  | { readonly type: "agents"; readonly payload: AgentPageView }
  | { readonly type: "agent_defined"; readonly payload: AgentDefinedView }
  | { readonly type: "agent_undefined"; readonly payload: AgentUndefinedView }
  | { readonly type: "agent_started"; readonly payload: AgentView }
  | { readonly type: "agent_stopped"; readonly payload: AgentView }
  | { readonly type: "agent_tasks"; readonly payload: AgentTaskPageView }
  | { readonly type: "agent_task"; readonly payload: AgentTaskDetailsView }
  | { readonly type: "agent_task_events"; readonly payload: AgentEventPageView }
  | { readonly type: "agent_task_submitted"; readonly payload: AgentSubmittedView }
  | { readonly type: "agent_task_sent"; readonly payload: AgentTaskView }
  | { readonly type: "agent_task_cancelled"; readonly payload: AgentTaskView }

/** Ordered transport frame for one frozen logical snapshot. */
export type SnapshotFragment = {
  readonly type: "snapshot_fragment"
  readonly revision: number
  readonly runtime_boot_id: string
  readonly index: number
  readonly last: boolean
  readonly data_hex: string
}
