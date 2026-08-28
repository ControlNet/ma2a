export const LOCAL_API_VERSION = 1 as const
export const LOCAL_API_SCHEMA_SHA256 =
  "6703648a604f92cf5011b449ca42d4242fa9c6453b5ecf3a94c98ef98009dcf6"
export const LOCAL_API_SCHEMA_JSON = `{"schema":"ma2a.local-api","version":1,"compatibility":"exact","bounds":{"request_bytes":16384,"response_bytes":65536,"event_bytes":16384,"text_bytes":4096,"collection_items":256},"ids":{"endpoint_id":"validated-lowercase-hex-32-bytes","space_id":"lowercase-hex-32-bytes","request_id":"lowercase-hex-16-bytes"},"commands":["handshake","status","endpoint_info","space_create","space_list","space_show","space_invite","space_redeem","space_revoke","control_sync_status","control_sync_trigger","private_relay_configure","private_relay_status","public_relay_configure","public_relay_status","echo_call","ui_password_set","ui_password_reset","session_revoke_all","snapshot_fetch","graceful_shutdown"],"results":["handshake","status","endpoint_info","space_created","spaces","space","space_invitation_created","space_redeemed","space_revoked","control_sync_status","control_sync_triggered","private_relay_configured","private_relay_status","public_relay_configured","public_relay_status","echo","ui_password_set","ui_password_reset","sessions_revoked","snapshot","shutting_down"],"errors":["version_mismatch","invalid_input","unauthorized","not_found","conflict","expired","rollback","unavailable","internal"],"events":["snapshot_invalidated","endpoint_changed","spaces_changed","control_sync_changed","relay_candidates_changed","relay_state_changed","reachability_changed","echo_summary_changed","ui_auth_changed"],"snapshot_fields":["revision","endpoint","spaces","control_sync","relay_candidates","observed_relay_state","reachability","recent_echo_summary","ui_auth"],"handshake_fields":["runtime_version","endpoint_id","revision","initialized","password_set","capabilities"],"request_id":{"same_payload":"replay","different_payload":"conflict","persistence":"out-of-scope"},"events_policy":{"delivery":"best-effort","durable_replay":false,"gap":"resnapshot","disconnect":"resnapshot"}}`

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
      readonly peer_endpoint_id: EndpointId
    })
  | (Mutation & { readonly operation: "space_redeem"; readonly invitation: string })
  | (Mutation & {
      readonly operation: "space_revoke"
      readonly space_id: SpaceId
      readonly peer_endpoint_id: EndpointId
    })
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
      readonly host: string
      readonly port: number
    })
  | (Versioned & { readonly operation: "private_relay_status" })
  | (Mutation & { readonly operation: "public_relay_configure"; readonly url: string })
  | (Versioned & { readonly operation: "public_relay_status" })
  | (Mutation & {
      readonly operation: "echo_call"
      readonly target_endpoint_id: EndpointId
      readonly payload: string
    })
  | (Mutation & { readonly operation: "ui_password_set"; readonly password: string })
  | (Mutation & { readonly operation: "ui_password_reset"; readonly password: string })
  | (Mutation & { readonly operation: "session_revoke_all" })
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
    case "control_sync_status":
    case "control_sync_trigger":
    case "private_relay_configure":
    case "private_relay_status":
    case "public_relay_configure":
    case "public_relay_status":
    case "echo_call":
    case "ui_password_set":
    case "ui_password_reset":
    case "session_revoke_all":
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
