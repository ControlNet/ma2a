import type { CommandResult, LocalApiCommand } from "./generated"

/** Exhaustive discriminants of every local API command and result. */
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
    case "space_grant":
    case "service_expose":
    case "service_unexpose":
    case "service_exposure_list":
    case "operations_status":
    case "stream_open":
    case "process_list":
    case "process_inspect":
    case "process_terminate":
    case "process_delete":
    case "agent_define":
    case "agent_undefine":
    case "agent_start":
    case "agent_stop":
    case "agent_list":
    case "agent_task_list":
    case "agent_task_inspect":
    case "agent_task_events":
    case "agent_task_submit":
    case "agent_task_send":
    case "agent_task_cancel":
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
    case "space_granted":
    case "service_exposed":
    case "service_unexposed":
    case "service_exposures":
    case "operations":
    case "stream_opened":
    case "processes":
    case "process":
    case "process_terminated":
    case "process_deleted":
    case "agents":
    case "agent_defined":
    case "agent_undefined":
    case "agent_started":
    case "agent_stopped":
    case "agent_tasks":
    case "agent_task":
    case "agent_task_events":
    case "agent_task_submitted":
    case "agent_task_sent":
    case "agent_task_cancelled":
      return result.type
    default:
      return assertNever(result)
  }
}

class LocalApiExhaustivenessError extends Error {
  readonly name = "LocalApiExhaustivenessError"
}

export function assertNever(value: never): never {
  throw new LocalApiExhaustivenessError(`unexpected local API variant: ${JSON.stringify(value)}`)
}
