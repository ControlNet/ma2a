import type { EndpointId, LOCAL_API_VERSION, ProcessId, RequestId } from "./generated"

type Versioned = { readonly version: typeof LOCAL_API_VERSION }
type Mutation = Versioned & { readonly request_id: RequestId }

/** Phase 3 local operator commands on the Processes this Runtime hosts. */
export type ProcessCommand =
  | (Versioned & { readonly operation: "process_list"; readonly after: ProcessId | null })
  | (Versioned & { readonly operation: "process_inspect"; readonly process_id: ProcessId })
  | (Mutation & {
      readonly operation: "process_terminate"
      readonly process_id: ProcessId
      readonly grace_ms: number
    })
  | (Mutation & { readonly operation: "process_delete"; readonly process_id: ProcessId })

export type ProcessStateName =
  | "created"
  | "starting"
  | "running"
  | "exited"
  | "terminated"
  | "failed"
  | "lost"
export type ProcessReasonName =
  | "spawn_failed"
  | "interrupted"
  | "requested"
  | "unauthorized"
  | "supervisor_lost"

/** Content-free summary of one hosted Process; archived ones are tombstones. */
export type ProcessSummaryView = {
  readonly process_id: ProcessId
  readonly creator_endpoint_id: EndpointId
  readonly request_id: RequestId
  readonly name: string | null
  readonly state: ProcessStateName
  readonly reason: ProcessReasonName | null
  readonly exit_code: number | null
  readonly signal: number | null
  readonly created_at_ms: number
  readonly started_at_ms: number | null
  readonly ended_at_ms: number | null
  readonly archived: boolean
}

export type ProcessLabelView = { readonly key: string; readonly value: string }

/** The start a Process was created with; environment values are never shown. */
export type ProcessStartView = {
  readonly program: string
  readonly args: readonly string[]
  readonly args_elided: boolean
  readonly cwd: string | null
  readonly env_names: readonly string[]
  readonly stdin: boolean
  readonly log_limit: number
  readonly labels: readonly ProcessLabelView[]
}

export type ProcessDetailsView = {
  readonly process: ProcessSummaryView
  readonly start: ProcessStartView | null
  readonly log_first_available: number
  readonly log_end: number
}

export type ProcessPageView = {
  readonly processes: readonly ProcessSummaryView[]
  readonly next: ProcessId | null
}
