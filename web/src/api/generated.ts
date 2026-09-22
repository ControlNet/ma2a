export const LOCAL_API_VERSION = 1 as const
export const LOCAL_API_SCHEMA_SHA256 =
  "f09b77a5d1809d93809be557ac1554fe06b5e7304d38b497a4886b0cd330497e"
export const LOCAL_API_SCHEMA_JSON = `{"schema":"ma2a.local-api","version":1,"compatibility":"exact","object_policy":{"listed_fields":"required","additional_fields":false,"duplicate_fields":"invalid_input"},"bounds":{"request_bytes":16384,"response_bytes":65536,"event_bytes":16384,"text_bytes":4096,"collection_items":256},"types":{"endpoint_id":{"type":"string","format":"lowercase_hex","bytes":32},"space_id":{"type":"string","format":"lowercase_hex","bytes":32},"request_id":{"type":"string","format":"lowercase_hex","bytes":16},"revision":{"type":"integer","wire":"u64","minimum":0},"u32":{"type":"integer","wire":"u32","minimum":0,"maximum":4294967295},"boolean":{"type":"boolean"},"runtime_version":{"type":"string","min_bytes":1,"max_bytes":128},"space_name":{"type":"string","min_bytes":1,"max_bytes":128},"relay_mode":{"type":"enum","values":["native_tls","external_termination"]},"endpoint":{"type":"object","required":["endpoint_id","runtime_version","online"],"fields":{"endpoint_id":{"ref":"endpoint_id"},"runtime_version":{"ref":"runtime_version"},"online":{"ref":"boolean"}}},"space":{"type":"object","required":["space_id","name","member_count"],"fields":{"space_id":{"ref":"space_id"},"name":{"ref":"space_name"},"member_count":{"ref":"u32"}}},"space_identity":{"type":"object","required":["space_id","name"],"fields":{"space_id":{"ref":"space_id"},"name":{"ref":"space_name"}}},"space_member":{"type":"object","required":["endpoint_id","label","echo","relay_provider"],"fields":{"endpoint_id":{"ref":"endpoint_id"},"label":{"type":"string","min_bytes":1,"max_bytes":64},"echo":{"ref":"boolean"},"relay_provider":{"ref":"boolean"}}},"snapshot_space":{"type":"object","required":["space_id","name","member_count","generation","chain_hash","revoked_count"],"fields":{"space_id":{"ref":"space_id"},"name":{"ref":"space_name"},"member_count":{"ref":"u32"},"generation":{"ref":"revision"},"chain_hash":{"type":"string","format":"lowercase_hex","bytes":32},"revoked_count":{"ref":"u32"}}},"connection_observation":{"type":"object","required":["observed_at_ms","path","rtt_ms","error_class"],"fields":{"observed_at_ms":{"type":"integer","wire":"u64","minimum":0},"path":{"type":"enum","values":["connecting","direct","relay","mixed_or_unknown"]},"rtt_ms":{"nullable":{"type":"integer","wire":"u64","minimum":0}},"error_class":{"type":"enum","values":["none","transient","authorization","version","revocation","malformed_input","policy","cancelled"]}}},"control_sync":{"type":"object","required":["peer_endpoint_ids"],"fields":{"peer_endpoint_ids":{"type":"array","max_items":256,"items":{"ref":"endpoint_id"}}}},"connection":{"type":"object","required":["endpoint_id","state","path","rtt_ms","observations"],"fields":{"endpoint_id":{"ref":"endpoint_id"},"state":{"type":"enum","values":["connecting","connected","failed"]},"path":{"type":"enum","values":["direct","relay","mixed_or_unknown"]},"rtt_ms":{"nullable":{"type":"integer","wire":"u64","minimum":0}},"observations":{"type":"array","max_items":8,"items":{"ref":"connection_observation"}}}},"private_relay_candidate":{"type":"object","required":["provider_endpoint_id","relay_url","covered_space_ids","home_compatible"],"fields":{"provider_endpoint_id":{"ref":"endpoint_id"},"relay_url":{"type":"string","min_bytes":1,"max_bytes":2048},"covered_space_ids":{"type":"array","max_items":256,"items":{"ref":"space_id"}},"home_compatible":{"ref":"boolean"}}},"public_relay_fallback":{"type":"object","required":["relay_url","enabled","observed_connected"],"fields":{"relay_url":{"type":"string","min_bytes":1,"max_bytes":2048},"enabled":{"ref":"boolean"},"observed_connected":{"ref":"boolean"}}},"control_round":{"type":"object","required":["at_ms","peer_count","outcome"],"fields":{"at_ms":{"type":"integer","wire":"u64","minimum":0},"peer_count":{"ref":"u32"},"outcome":{"type":"enum","values":["succeeded","failed","empty"]}}},"observed_relay_state":{"type":"object","required":["private_relay_provider_running","public_relay_connected"],"fields":{"private_relay_provider_running":{"ref":"boolean"},"public_relay_connected":{"ref":"boolean"}}},"reachability":{"type":"object","required":["state","direct","relayed"],"fields":{"state":{"type":"enum","values":["NoActiveSpaces","DegradedNoCommonHome","AwaitingIrohHome","IrohHomeConnected"]},"direct":{"ref":"boolean"},"relayed":{"ref":"boolean"}}},"echo_summary":{"type":"object","required":["successes","failures"],"fields":{"successes":{"ref":"u32"},"failures":{"ref":"u32"}}},"ui_auth":{"type":"object","required":["initialized","password_set","active_sessions"],"fields":{"initialized":{"ref":"boolean"},"password_set":{"ref":"boolean"},"active_sessions":{"ref":"u32"}}},"capabilities":{"type":"object","required":["spaces","control_sync","private_relay","public_relay","echo","events"],"fields":{"spaces":{"ref":"boolean"},"control_sync":{"ref":"boolean"},"private_relay":{"ref":"boolean"},"public_relay":{"ref":"boolean"},"echo":{"ref":"boolean"},"events":{"ref":"boolean"}}},"handshake":{"type":"object","required":["runtime_version","endpoint_id","revision","initialized","password_set","capabilities"],"fields":{"runtime_version":{"ref":"runtime_version"},"endpoint_id":{"ref":"endpoint_id"},"revision":{"ref":"revision"},"initialized":{"ref":"boolean"},"password_set":{"ref":"boolean"},"capabilities":{"ref":"capabilities"}}},"status":{"type":"object","required":["revision","initialized","shutting_down"],"fields":{"revision":{"ref":"revision"},"initialized":{"ref":"boolean"},"shutting_down":{"ref":"boolean"}}},"private_relay":{"type":"object","required":["configured","mode","host","port","online"],"fields":{"configured":{"ref":"boolean"},"mode":{"ref":"relay_mode"},"host":{"type":"string","min_bytes":1,"max_bytes":253},"port":{"type":"integer","wire":"u16","minimum":0,"maximum":65535},"online":{"ref":"boolean"}}},"public_relay":{"type":"object","required":["configured","url","online"],"fields":{"configured":{"ref":"boolean"},"url":{"nullable":{"type":"string","max_bytes":2048,"format":"https_url_without_credentials"}},"online":{"ref":"boolean"}}},"echo_reply":{"type":"object","required":["target_endpoint_id","payload","duration_ms"],"fields":{"target_endpoint_id":{"ref":"endpoint_id"},"payload":{"type":"string","max_bytes":4096},"duration_ms":{"type":"integer","wire":"u16","minimum":0,"maximum":10000}}},"runtime_snapshot":{"type":"object","required":["revision","endpoint","spaces","control_sync","connections","private_relay_candidates","public_relay_fallbacks","control_rounds","observed_relay_state","reachability","recent_echo_summary","ui_auth"],"fields":{"revision":{"ref":"revision"},"endpoint":{"ref":"endpoint"},"spaces":{"type":"array","max_items":256,"items":{"ref":"snapshot_space"}},"control_sync":{"ref":"control_sync"},"connections":{"type":"array","max_items":256,"items":{"ref":"connection"}},"private_relay_candidates":{"type":"array","max_items":256,"items":{"ref":"private_relay_candidate"}},"public_relay_fallbacks":{"type":"array","max_items":256,"items":{"ref":"public_relay_fallback"}},"control_rounds":{"type":"array","max_items":16,"items":{"ref":"control_round"}},"observed_relay_state":{"ref":"observed_relay_state"},"reachability":{"ref":"reachability"},"recent_echo_summary":{"ref":"echo_summary"},"ui_auth":{"ref":"ui_auth"}}},"changed_endpoint_ids":{"type":"object","required":["endpoint_ids"],"fields":{"endpoint_ids":{"type":"array","max_items":256,"items":{"ref":"endpoint_id"}}}},"changed_space_ids":{"type":"object","required":["space_ids"],"fields":{"space_ids":{"type":"array","max_items":256,"items":{"ref":"space_id"}}}},"empty_object":{"type":"object","required":[],"fields":{}},"path":{"type":"string","min_bytes":1,"max_bytes":4096},"space_details":{"type":"object","required":["revision","space","members"],"fields":{"revision":{"ref":"revision"},"space":{"ref":"snapshot_space"},"members":{"type":"array","max_items":64,"items":{"ref":"space_member"}}}},"snapshot_stamp":{"type":"object","required":["revision","runtime_boot_id"],"fields":{"revision":{"ref":"revision"},"runtime_boot_id":{"type":"string","format":"lowercase_hex","bytes":16}}},"ui_status":{"type":"object","required":["running","url"],"fields":{"running":{"ref":"boolean"},"url":{"nullable":{"type":"string","min_bytes":1,"max_bytes":64}}}}},"commands":{"handshake":{"required":[],"fields":{}},"status":{"required":[],"fields":{}},"endpoint_info":{"required":[],"fields":{}},"space_create":{"required":["request_id","name"],"fields":{"request_id":{"ref":"request_id"},"name":{"ref":"space_name"}}},"space_list":{"required":[],"fields":{}},"space_show":{"required":["space_id"],"fields":{"space_id":{"ref":"space_id"}}},"space_invite":{"required":["request_id","space_id","ttl_ms","output_path"],"fields":{"request_id":{"ref":"request_id"},"space_id":{"ref":"space_id"},"ttl_ms":{"type":"integer","wire":"u64","minimum":1,"maximum":300000},"output_path":{"ref":"path"}}},"space_redeem":{"required":["request_id","invitation"],"fields":{"request_id":{"ref":"request_id"},"invitation":{"type":"string","min_bytes":1,"max_bytes":2048}}},"space_revoke":{"required":["request_id","space_id","peer_endpoint_id"],"fields":{"request_id":{"ref":"request_id"},"space_id":{"ref":"space_id"},"peer_endpoint_id":{"ref":"endpoint_id"}}},"space_leave":{"required":["request_id","space_id"],"fields":{"request_id":{"ref":"request_id"},"space_id":{"ref":"space_id"}}},"control_sync_status":{"required":["peer_endpoint_id"],"fields":{"peer_endpoint_id":{"ref":"endpoint_id"}}},"control_sync_trigger":{"required":["request_id","peer_endpoint_id"],"fields":{"request_id":{"ref":"request_id"},"peer_endpoint_id":{"ref":"endpoint_id"}}},"private_relay_configure":{"required":["request_id","mode","listen","public_url","served_space_ids","certificate_path","private_key_path"],"fields":{"request_id":{"ref":"request_id"},"mode":{"ref":"relay_mode"},"listen":{"type":"string","min_bytes":1,"max_bytes":128},"public_url":{"type":"string","min_bytes":1,"max_bytes":2048,"format":"https_url_without_credentials"},"served_space_ids":{"type":"array","max_items":256,"items":{"ref":"space_id"}},"certificate_path":{"nullable":{"ref":"path"}},"private_key_path":{"nullable":{"ref":"path"}}}},"private_relay_status":{"required":[],"fields":{}},"public_relay_configure":{"required":["request_id","url"],"fields":{"request_id":{"ref":"request_id"},"url":{"type":"string","min_bytes":1,"max_bytes":2048,"format":"https_url_without_credentials"}}},"public_relay_status":{"required":[],"fields":{}},"echo_call":{"required":["request_id","target_endpoint_id","payload"],"fields":{"request_id":{"ref":"request_id"},"target_endpoint_id":{"ref":"endpoint_id"},"payload":{"type":"string","min_bytes":1,"max_bytes":4096}}},"ui_password_set":{"required":["request_id","password"],"fields":{"request_id":{"ref":"request_id"},"password":{"type":"string","min_bytes":1,"max_bytes":1024}}},"ui_password_reset":{"required":["request_id","password"],"fields":{"request_id":{"ref":"request_id"},"password":{"type":"string","min_bytes":1,"max_bytes":1024}}},"session_revoke_all":{"required":["request_id"],"fields":{"request_id":{"ref":"request_id"}}},"snapshot_fetch":{"required":[],"fields":{}},"graceful_shutdown":{"required":["request_id"],"fields":{"request_id":{"ref":"request_id"}}},"private_relay_disable":{"required":["request_id"],"fields":{"request_id":{"ref":"request_id"}}},"public_relay_disable":{"required":["request_id"],"fields":{"request_id":{"ref":"request_id"}}},"space_details_fetch":{"required":["space_id"],"fields":{"space_id":{"ref":"space_id"}}},"snapshot_stamp":{"required":[],"fields":{}},"ui_init":{"required":["request_id","password"],"fields":{"request_id":{"ref":"request_id"},"password":{"type":"string","min_bytes":1,"max_bytes":1024}}},"ui_start":{"required":["host","port"],"fields":{"host":{"type":"string","min_bytes":1,"max_bytes":253,"format":"ip_or_hostname"},"port":{"type":"integer","wire":"u16","minimum":0,"maximum":65535}}},"ui_stop":{"required":[],"fields":{}},"ui_status":{"required":[],"fields":{}}},"responses":{"success":{"required":["version","request_id","revision","result"],"fields":{"version":{"literal":1},"request_id":{"nullable":{"ref":"request_id"}},"revision":{"ref":"revision"},"result":{"one_of":"results"}}},"error":{"required":["version","error","remediation"],"fields":{"version":{"literal":1},"error":{"type":"enum","values":["version_mismatch","invalid_input","unauthorized","not_found","conflict","expired","rollback","unavailable","internal"]},"remediation":{"nullable":{"type":"string","max_bytes":128}}}}},"results":{"handshake":{"payload":{"ref":"handshake"}},"status":{"payload":{"ref":"status"}},"endpoint_info":{"payload":{"ref":"endpoint"}},"space_created":{"payload":{"ref":"space"}},"spaces":{"payload":{"type":"array","max_items":256,"items":{"ref":"space"}}},"space":{"payload":{"ref":"space"}},"space_invitation_created":{"payload":{"ref":"space"}},"space_redeemed":{"payload":{"ref":"space"}},"space_revoked":{"payload":{"ref":"space"}},"space_left":{"payload":{"ref":"space_identity"}},"control_sync_status":{"payload":{"ref":"control_sync"}},"control_sync_triggered":{"payload":{"ref":"control_sync"}},"private_relay_configured":{"payload":{"ref":"private_relay"}},"private_relay_status":{"payload":{"ref":"private_relay"}},"public_relay_configured":{"payload":{"ref":"public_relay"}},"public_relay_status":{"payload":{"ref":"public_relay"}},"echo":{"payload":{"ref":"echo_reply"}},"ui_password_set":{"payload":{"ref":"ui_auth"}},"ui_password_reset":{"payload":{"ref":"ui_auth"}},"sessions_revoked":{"payload":{"ref":"ui_auth"}},"snapshot":{"payload":{"ref":"runtime_snapshot"}},"shutting_down":{"payload":{"ref":"empty_object"}},"space_details":{"payload":{"ref":"space_details"}},"snapshot_stamp":{"payload":{"ref":"snapshot_stamp"}},"ui_initialized":{"payload":{"ref":"ui_auth"}},"ui_status":{"payload":{"ref":"ui_status"}}},"events":{"snapshot_invalidated":{"fields":{"changed":{"ref":"empty_object"}}},"endpoint_changed":{"fields":{"changed":{"ref":"changed_endpoint_ids"}}},"spaces_changed":{"fields":{"changed":{"ref":"changed_space_ids"}}},"control_sync_changed":{"fields":{"changed":{"ref":"changed_endpoint_ids"}}},"relay_candidates_changed":{"fields":{"changed":{"ref":"changed_endpoint_ids"}}},"relay_state_changed":{"fields":{"changed":{"ref":"empty_object"}}},"reachability_changed":{"fields":{"changed":{"ref":"changed_endpoint_ids"}}},"echo_summary_changed":{"fields":{"changed":{"ref":"changed_endpoint_ids"}}},"ui_auth_changed":{"fields":{"changed":{"ref":"empty_object"}}}},"request_id_policy":{"same_payload":"replay","different_payload":"conflict","persistence":"out-of-scope"},"events_policy":{"delivery":"best-effort","durable_replay":false,"gap":"resnapshot","disconnect":"resnapshot"},"privacy":{"excluded":["authorized_via","private_key","invite_secret","session_bearer","password_verifier"]},"ui_lifecycle_policy":{"commands":["ui_start","ui_stop","ui_status"],"persistence":false,"request_id":false,"replay":false,"start":"restart","initial_state":"stopped"}}`

import type { CommandResult } from "./generated-models"

export type * from "./generated-models"

declare const brand: unique symbol
type Brand<T, Name extends string> = T & { readonly [brand]: Name }

export type EndpointId = Brand<string, "EndpointId">
export type SpaceId = Brand<string, "SpaceId">
export type RequestId = Brand<string, "RequestId">

type Versioned = { readonly version: typeof LOCAL_API_VERSION }
type Mutation = Versioned & { readonly request_id: RequestId }

export type LocalApiCommand =
  | (Versioned & { readonly operation: "handshake" })
  | (Versioned & { readonly operation: "status" })
  | (Versioned & { readonly operation: "endpoint_info" })
  | (Mutation & { readonly operation: "space_create"; readonly name: string })
  | (Versioned & { readonly operation: "space_list" })
  | (Versioned & { readonly operation: "space_show"; readonly space_id: SpaceId })
  | (Mutation & {
      readonly operation: "space_invite"
      readonly space_id: SpaceId
      readonly ttl_ms: number
      readonly output_path: string
    })
  | (Mutation & { readonly operation: "space_redeem"; readonly invitation: string })
  | (Mutation & {
      readonly operation: "space_revoke"
      readonly space_id: SpaceId
      readonly peer_endpoint_id: EndpointId
    })
  | (Mutation & { readonly operation: "space_leave"; readonly space_id: SpaceId })
  | (Versioned & {
      readonly operation: "control_sync_status"
      readonly peer_endpoint_id: EndpointId
    })
  | (Mutation & {
      readonly operation: "control_sync_trigger"
      readonly peer_endpoint_id: EndpointId
    })
  | (Mutation & {
      readonly operation: "private_relay_configure"
      readonly mode: "native_tls" | "external_termination"
      readonly listen: string
      readonly public_url: string
      readonly served_space_ids: readonly SpaceId[]
      readonly certificate_path: string | null
      readonly private_key_path: string | null
    })
  | (Mutation & { readonly operation: "private_relay_disable" })
  | (Versioned & { readonly operation: "private_relay_status" })
  | (Mutation & { readonly operation: "public_relay_configure"; readonly url: string })
  | (Mutation & { readonly operation: "public_relay_disable" })
  | (Versioned & { readonly operation: "public_relay_status" })
  | (Mutation & {
      readonly operation: "echo_call"
      readonly target_endpoint_id: EndpointId
      readonly payload: string
    })
  | (Mutation & { readonly operation: "ui_password_set"; readonly password: string })
  | (Mutation & { readonly operation: "ui_password_reset"; readonly password: string })
  | (Mutation & { readonly operation: "session_revoke_all" })
  | (Mutation & { readonly operation: "ui_init"; readonly password: string })
  | (Versioned & { readonly operation: "ui_start"; readonly host: string; readonly port: number })
  | (Versioned & { readonly operation: "ui_stop" })
  | (Versioned & { readonly operation: "ui_status" })
  | (Versioned & { readonly operation: "space_details_fetch"; readonly space_id: SpaceId })
  | (Versioned & { readonly operation: "snapshot_stamp" })
  | (Versioned & { readonly operation: "snapshot_fetch" })
  | (Mutation & { readonly operation: "graceful_shutdown" })

export const ERROR_CODES = [
  "version_mismatch",
  "invalid_input",
  "unauthorized",
  "not_found",
  "conflict",
  "expired",
  "rollback",
  "unavailable",
  "internal",
] as const
export type LocalApiErrorCode = (typeof ERROR_CODES)[number]

export type LocalApiSuccessResponse = {
  readonly version: typeof LOCAL_API_VERSION
  readonly request_id: RequestId | null
  readonly revision: number
  readonly result: CommandResult
}

export type LocalApiErrorResponse = {
  readonly version: typeof LOCAL_API_VERSION
  readonly error: LocalApiErrorCode
  readonly remediation: string | null
}

export type LocalApiResponse = LocalApiSuccessResponse | LocalApiErrorResponse

export function errorCode(error: LocalApiErrorCode): string {
  switch (error) {
    case "version_mismatch":
    case "invalid_input":
    case "unauthorized":
    case "not_found":
    case "conflict":
    case "expired":
    case "rollback":
    case "unavailable":
    case "internal":
      return error
    default:
      return assertNever(error)
  }
}

type EndpointChangedEvent<Type extends string> = {
  readonly type: Type
  readonly revision: number
  readonly changed: { readonly endpoint_ids: readonly EndpointId[] }
}

export type RuntimeEvent =
  | {
      readonly type: "snapshot_invalidated"
      readonly revision: number
      readonly changed: Record<string, never>
    }
  | EndpointChangedEvent<"endpoint_changed">
  | {
      readonly type: "spaces_changed"
      readonly revision: number
      readonly changed: { readonly space_ids: readonly SpaceId[] }
    }
  | EndpointChangedEvent<"control_sync_changed">
  | EndpointChangedEvent<"relay_candidates_changed">
  | {
      readonly type: "relay_state_changed"
      readonly revision: number
      readonly changed: Record<string, never>
    }
  | EndpointChangedEvent<"reachability_changed">
  | EndpointChangedEvent<"echo_summary_changed">
  | {
      readonly type: "ui_auth_changed"
      readonly revision: number
      readonly changed: Record<string, never>
    }

export function commandOperation(command: LocalApiCommand): string {
  switch (command.operation) {
    case "handshake":
    case "status":
    case "endpoint_info":
    case "space_create":
    case "space_list":
    case "space_show":
    case "space_invite":
    case "space_redeem":
    case "space_revoke":
    case "space_leave":
    case "control_sync_status":
    case "control_sync_trigger":
    case "private_relay_configure":
    case "private_relay_disable":
    case "private_relay_status":
    case "public_relay_configure":
    case "public_relay_disable":
    case "public_relay_status":
    case "echo_call":
    case "ui_password_set":
    case "ui_password_reset":
    case "session_revoke_all":
    case "ui_init":
    case "ui_start":
    case "ui_stop":
    case "ui_status":
    case "space_details_fetch":
    case "snapshot_stamp":
    case "snapshot_fetch":
    case "graceful_shutdown":
      return command.operation
    default:
      return assertNever(command)
  }
}

export function resultType(result: CommandResult): string {
  switch (result.type) {
    case "handshake":
    case "status":
    case "endpoint_info":
    case "space_created":
    case "spaces":
    case "space":
    case "space_invitation_created":
    case "space_redeemed":
    case "space_revoked":
    case "space_left":
    case "control_sync_status":
    case "control_sync_triggered":
    case "private_relay_configured":
    case "private_relay_status":
    case "public_relay_configured":
    case "public_relay_status":
    case "echo":
    case "ui_password_set":
    case "ui_password_reset":
    case "sessions_revoked":
    case "ui_initialized":
    case "ui_status":
    case "space_details":
    case "snapshot_stamp":
    case "snapshot":
    case "shutting_down":
      return result.type
    default:
      return assertNever(result)
  }
}

export function eventRequiresResnapshot(previousRevision: number, event: RuntimeEvent): boolean {
  switch (event.type) {
    case "snapshot_invalidated":
      return true
    case "endpoint_changed":
    case "spaces_changed":
    case "control_sync_changed":
    case "relay_candidates_changed":
    case "relay_state_changed":
    case "reachability_changed":
    case "echo_summary_changed":
    case "ui_auth_changed":
      return event.revision !== previousRevision + 1
    default:
      return assertNever(event)
  }
}

class LocalApiExhaustivenessError extends Error {
  readonly name = "LocalApiExhaustivenessError"
}

function assertNever(value: never): never {
  throw new LocalApiExhaustivenessError(`unexpected local API variant: ${JSON.stringify(value)}`)
}
