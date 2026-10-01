import type { AgentTaskId, EndpointId, LOCAL_API_VERSION, ProcessId, RequestId } from "./generated"

type Versioned = { readonly version: typeof LOCAL_API_VERSION }
type Mutation = Versioned & { readonly request_id: RequestId }

export type AgentToolFamily = "files" | "transfer" | "exec" | "process" | "proxy" | "agent"

export type AgentMessagePart = { readonly kind: "text" | "data"; readonly content: string }

/** One bounded message; `data` parts carry JSON text. */
export type AgentMessage = {
  readonly role: "user" | "agent"
  readonly parts: readonly AgentMessagePart[]
}

export type AgentEnvVar = { readonly name: string; readonly value: string }

/** Phase 4 local operator commands on the Agents this Runtime hosts and their tasks. */
export type AgentCommand =
  | (Mutation & {
      readonly operation: "agent_define"
      readonly agent: string
      readonly description: string
      readonly program: string
      readonly args: readonly string[]
      readonly cwd: string | null
      readonly env: readonly AgentEnvVar[]
      readonly tools: readonly AgentToolFamily[]
      readonly max_tasks: number
    })
  | (Mutation & { readonly operation: "agent_undefine"; readonly agent: string })
  | (Mutation & { readonly operation: "agent_start"; readonly agent: string })
  | (Mutation & { readonly operation: "agent_stop"; readonly agent: string })
  | (Versioned & { readonly operation: "agent_list"; readonly after: string | null })
  | (Versioned & {
      readonly operation: "agent_task_list"
      readonly agent: string | null
      readonly after: AgentTaskId | null
    })
  | (Versioned & { readonly operation: "agent_task_inspect"; readonly task_id: AgentTaskId })
  | (Versioned & {
      readonly operation: "agent_task_events"
      readonly task_id: AgentTaskId
      readonly after: number
    })
  | (Versioned & {
      readonly operation: "agent_task_artifact_read"
      readonly task_id: AgentTaskId
      readonly name: string
      readonly offset: number
    })
  | (Mutation & {
      readonly operation: "agent_task_submit"
      readonly agent: string
      readonly message: AgentMessage
    })
  | (Mutation & {
      readonly operation: "agent_task_send"
      readonly task_id: AgentTaskId
      readonly message: AgentMessage
    })
  | (Mutation & { readonly operation: "agent_task_cancel"; readonly task_id: AgentTaskId })

export type AgentTaskStateName =
  | "queued"
  | "running"
  | "waiting"
  | "completed"
  | "failed"
  | "cancelled"
export type AgentTaskReasonName =
  | "requested"
  | "unauthorized"
  | "agent_failed"
  | "agent_lost"
  | "agent_stopped"
  | "limit"

export type AgentInstanceView = {
  readonly generation: number
  readonly process_id: ProcessId | null
  readonly ready: boolean
  readonly started_at_ms: number
  readonly ended_at_ms: number | null
}

/** One defined Agent; environment values are never shown, and `elided` drops args and names. */
export type AgentView = {
  readonly agent: string
  readonly description: string
  readonly program: string | null
  readonly args: readonly string[]
  readonly env_names: readonly string[]
  readonly elided: boolean
  readonly cwd: string | null
  readonly tools: readonly AgentToolFamily[]
  readonly max_tasks: number
  readonly enabled: boolean
  readonly generation: number
  readonly defined_at_ms: number
  readonly changed_at_ms: number
  readonly live_tasks: number
  readonly instance: AgentInstanceView | null
}

export type AgentPageView = { readonly agents: readonly AgentView[]; readonly next: string | null }
export type AgentDefinedView = { readonly agent: AgentView; readonly created: boolean }
export type AgentUndefinedView = { readonly agent: string }

/** The content-free view of one task. */
export type AgentTaskView = {
  readonly task_id: AgentTaskId
  readonly agent: string
  readonly creator_endpoint_id: EndpointId
  readonly from_agent: string | null
  readonly request_id: RequestId
  readonly state: AgentTaskStateName
  readonly reason: AgentTaskReasonName | null
  readonly created_at_ms: number
  readonly started_at_ms: number | null
  readonly ended_at_ms: number | null
  readonly last_seq: number
}

export type AgentTaskPageView = {
  readonly tasks: readonly AgentTaskView[]
  readonly next: AgentTaskId | null
}

export type AgentArtifactView = {
  readonly name: string
  readonly media_type: string
  readonly size: number
  readonly blake3: string
}

/** One task in full; an oversized message is elided and flagged. */
export type AgentTaskDetailsView = {
  readonly task: AgentTaskView
  readonly input: AgentMessage | null
  readonly input_elided: boolean
  readonly result: AgentMessage | null
  readonly result_elided: boolean
  readonly error: string | null
  readonly progress: string | null
  readonly artifacts: readonly AgentArtifactView[]
}

/** One retained event; only the fields of its `kind` are set. */
export type AgentEventView = {
  readonly seq: number
  readonly at_ms: number
  readonly kind: "state" | "message" | "progress" | "artifact"
  readonly state: AgentTaskStateName | null
  readonly reason: AgentTaskReasonName | null
  readonly message: AgentMessage | null
  readonly message_elided: boolean
  readonly text: string | null
  readonly artifact: AgentArtifactView | null
}

export type AgentEventPageView = {
  readonly events: readonly AgentEventView[]
  readonly more: boolean
}

/** One bounded chunk of an artifact from `offset`; `content` is standard base64. */
export type AgentArtifactChunkView = {
  readonly artifact: AgentArtifactView
  readonly offset: number
  readonly content: string
  readonly eof: boolean
}

export type AgentSubmittedView = { readonly task: AgentTaskView; readonly existing: boolean }
