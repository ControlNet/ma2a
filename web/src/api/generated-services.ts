import type { EndpointId, LOCAL_API_VERSION, RequestId, SpaceId } from "./generated"

type Versioned = { readonly version: typeof LOCAL_API_VERSION }
type Mutation = Versioned & { readonly request_id: RequestId }
type ExposureSelector = {
  readonly space_id: SpaceId
  readonly kind: ExposureKind
  readonly root: string | null
  readonly host: string | null
  readonly port_start: number | null
  readonly port_end: number | null
}

/** Phase 2 service grant, exposure, and operation status commands. */
export type ServiceCommand =
  | (Mutation & {
      readonly operation: "space_grant"
      readonly space_id: SpaceId
      readonly peer_endpoint_id: EndpointId
      readonly files_read: boolean
      readonly files_write: boolean
      readonly exec: boolean
      readonly shell: boolean
      readonly proxy: boolean
      readonly process: boolean
    })
  | (Mutation &
      ExposureSelector & {
        readonly operation: "service_expose"
        readonly path: string | null
        readonly access: RootAccess | null
      })
  | (Mutation & ExposureSelector & { readonly operation: "service_unexpose" })
  | (Versioned & { readonly operation: "service_exposure_list" })
  | (Versioned & { readonly operation: "operations_status" })
  | (Mutation & {
      readonly operation: "stream_open"
      readonly target_endpoint_id: EndpointId
      readonly service: OperationService
    })

export type ExposureKind = "files_root" | "exec" | "shell" | "proxy" | "process"
export type RootAccess = "read_only" | "read_write"

/** One thing this Runtime serves inside one Space; inapplicable fields are null. */
export type ServiceExposureView = {
  readonly space_id: SpaceId
  readonly kind: ExposureKind
  readonly root: string | null
  readonly path: string | null
  readonly access: RootAccess | null
  readonly host: string | null
  readonly port_start: number | null
  readonly port_end: number | null
}

export type OperationService = "files" | "transfer" | "exec" | "shell" | "proxy" | "process"
export type OperationName =
  | "stat"
  | "list"
  | "mkdir"
  | "read"
  | "write"
  | "rename"
  | "remove"
  | "download"
  | "upload"
  | "exec"
  | "shell"
  | "proxy"
  | "process_start"
  | "process_list"
  | "process_inspect"
  | "process_logs"
  | "process_attach"
  | "process_stdin"
  | "process_signal"
  | "process_terminate"
  | "process_delete"
export type ServiceErrorName =
  | "version_mismatch"
  | "invalid_input"
  | "unauthorized"
  | "timed_out"
  | "concurrency_exceeded"
  | "cancelled"
  | "unavailable"
  | "internal"
  | "conflict"
  | "outcome_unknown"
  | "not_found"
  | "already_exists"
  | "not_a_directory"
  | "is_a_directory"
  | "not_empty"
  | "symlink_refused"
  | "invalid_path"
  | "permission_denied"
  | "read_only_root"
  | "too_large"
  | "io"
  | "integrity_mismatch"
  | "content_changed"
  | "precondition_failed"
  | "spawn_failed"
  | "connection_refused"
  | "unreachable"
  | "dns_failed"
  | "unknown_process"
  | "not_running"
  | "writer_busy"
  | "stdin_closed"
  | "process_active"

/** Content-free identity of one streaming operation. */
type OperationIdentityView = {
  readonly request_id: RequestId
  readonly peer_endpoint_id: EndpointId
  readonly service: OperationService
  readonly operation: OperationName
  readonly direction: "inbound" | "outbound"
}

export type ActiveOperationView = OperationIdentityView & { readonly elapsed_ms: number }

export type OperationRecordView = OperationIdentityView & {
  readonly result: "completed" | "rejected" | "failed"
  readonly error: ServiceErrorName | null
  readonly bytes_in: number
  readonly bytes_out: number
  readonly duration_ms: number
}

export type OperationsView = {
  readonly active_count: number
  readonly active: readonly ActiveOperationView[]
  readonly recent_count: number
  readonly recent: readonly OperationRecordView[]
}
