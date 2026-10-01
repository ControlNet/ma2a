import ky from "ky"
import { z } from "zod"

import { RuntimeApiPayloadError } from "./codec"

const HexId = (bytes: number) => z.string().regex(new RegExp(`^[0-9a-f]{${bytes * 2}}$`))
const Port = z.number().int().min(1).max(65_535)
const U64 = z.number().int().nonnegative()
const Service = z.enum(["files", "transfer", "exec", "shell", "proxy", "process", "agent"])
const Operation = z.enum([
  "stat",
  "list",
  "mkdir",
  "read",
  "write",
  "rename",
  "remove",
  "download",
  "upload",
  "exec",
  "shell",
  "proxy",
  "process_start",
  "process_list",
  "process_inspect",
  "process_logs",
  "process_attach",
  "process_stdin",
  "process_signal",
  "process_terminate",
  "process_delete",
  "agent_discover",
  "agent_submit",
  "agent_send",
  "agent_get",
  "agent_tasks",
  "agent_watch",
  "agent_cancel",
  "agent_artifact",
])
const ServiceError = z.enum([
  "version_mismatch",
  "invalid_input",
  "unauthorized",
  "timed_out",
  "concurrency_exceeded",
  "cancelled",
  "unavailable",
  "internal",
  "conflict",
  "outcome_unknown",
  "not_found",
  "already_exists",
  "not_a_directory",
  "is_a_directory",
  "not_empty",
  "symlink_refused",
  "invalid_path",
  "permission_denied",
  "read_only_root",
  "too_large",
  "io",
  "integrity_mismatch",
  "content_changed",
  "precondition_failed",
  "spawn_failed",
  "connection_refused",
  "unreachable",
  "dns_failed",
  "unknown_process",
  "not_running",
  "writer_busy",
  "stdin_closed",
  "process_active",
  "unknown_agent",
  "unknown_task",
  "agent_unavailable",
  "task_ended",
  "unknown_artifact",
])

const ExposureSchema = z.strictObject({
  space_id: HexId(32),
  kind: z.enum(["files_root", "exec", "shell", "proxy", "process", "agent"]),
  root: z.string().min(1).max(64).nullable(),
  path: z.string().min(1).max(4096).nullable(),
  access: z.enum(["read_only", "read_write"]).nullable(),
  host: z.string().min(1).max(253).nullable(),
  port_start: Port.nullable(),
  port_end: Port.nullable(),
  agent: z.string().min(1).max(63).nullable(),
})
const Identity = {
  request_id: HexId(16),
  peer_endpoint_id: HexId(32),
  service: Service,
  operation: Operation,
  direction: z.enum(["inbound", "outbound"]),
}
const OperationsSchema = z.strictObject({
  active_count: U64,
  active: z
    .array(z.strictObject({ ...Identity, elapsed_ms: U64 }))
    .max(64)
    .readonly(),
  recent_count: U64,
  recent: z
    .array(
      z.strictObject({
        ...Identity,
        result: z.enum(["completed", "rejected", "failed"]),
        error: ServiceError.nullable(),
        bytes_in: U64,
        bytes_out: U64,
        duration_ms: U64,
      }),
    )
    .max(64)
    .readonly(),
})

const ProcessSchema = z.strictObject({
  process_id: HexId(16),
  creator_endpoint_id: HexId(32),
  request_id: HexId(16),
  name: z.string().min(1).max(64).nullable(),
  state: z.enum(["created", "starting", "running", "exited", "terminated", "failed", "lost"]),
  reason: z
    .enum(["spawn_failed", "interrupted", "requested", "unauthorized", "supervisor_lost"])
    .nullable(),
  exit_code: z.number().int().nullable(),
  signal: z.number().int().nonnegative().nullable(),
  created_at_ms: U64,
  started_at_ms: U64.nullable(),
  ended_at_ms: U64.nullable(),
  archived: z.boolean(),
})
const ProcessPageSchema = z.strictObject({
  processes: z.array(ProcessSchema).max(64).readonly(),
  next: HexId(16).nullable(),
})

export type ServiceExposure = z.infer<typeof ExposureSchema>
export type HostedProcess = z.infer<typeof ProcessSchema>
export type ProcessPage = z.infer<typeof ProcessPageSchema>
export type Operations = z.infer<typeof OperationsSchema>
export type ActiveOperation = Operations["active"][number]
export type OperationRecord = Operations["recent"][number]

/** What this Runtime serves and what it is doing; content never reaches the browser. */
export type ServicesData = {
  readonly exposures: readonly ServiceExposure[]
  readonly operations: Operations
  readonly processes: ProcessPage
}

function parse<Value>(schema: z.ZodType<Value>, value: unknown): Value {
  const result = schema.safeParse(value)
  if (!result.success) {
    throw new RuntimeApiPayloadError(result.error.issues.map((issue) => issue.message))
  }
  return result.data
}

export function parseServicesData(
  exposures: unknown,
  operations: unknown,
  processes: unknown,
): ServicesData {
  return {
    exposures: parse(z.array(ExposureSchema).max(256).readonly(), exposures),
    operations: parse(OperationsSchema, operations),
    processes: parse(ProcessPageSchema, processes),
  }
}

export async function fetchServicesData(): Promise<ServicesData> {
  const http = ky.create({
    credentials: "same-origin",
    headers: { accept: "application/json" },
    retry: 0,
    timeout: 5_000,
  })
  const [exposures, operations, processes] = await Promise.all([
    http.get("/api/v1/services").json(),
    http.get("/api/v1/operations").json(),
    http.get("/api/v1/processes").json(),
  ])
  return parseServicesData(exposures, operations, processes)
}
