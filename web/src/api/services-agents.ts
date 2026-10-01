import { z } from "zod"

const HexId = (bytes: number) => z.string().regex(new RegExp(`^[0-9a-f]{${bytes * 2}}$`))
const U64 = z.number().int().nonnegative()

const AgentState = z.enum(["queued", "running", "waiting", "completed", "failed", "cancelled"])
const AgentReason = z.enum([
  "requested",
  "unauthorized",
  "agent_failed",
  "agent_lost",
  "agent_stopped",
  "limit",
])
const AgentName = z.string().min(1).max(63)
/** An Agent as the browser sees it: never its program, arguments, or environment. */
const AgentSchema = z.strictObject({
  agent: AgentName,
  description: z.string().max(256),
  tools: z
    .array(z.enum(["files", "transfer", "exec", "process", "proxy", "agent"]))
    .max(6)
    .readonly(),
  max_tasks: z.number().int().min(1).max(16),
  enabled: z.boolean(),
  generation: U64,
  live_tasks: U64,
  instance: z
    .strictObject({
      generation: U64,
      process_id: HexId(16).nullable(),
      ready: z.boolean(),
      started_at_ms: U64,
      ended_at_ms: U64.nullable(),
    })
    .nullable(),
})
/** A task as the browser sees it: never a message, progress text, or artifact. */
const AgentTaskSchema = z.strictObject({
  task_id: HexId(16),
  agent: AgentName,
  creator_endpoint_id: HexId(32),
  from_agent: AgentName.nullable(),
  request_id: HexId(16),
  state: AgentState,
  reason: AgentReason.nullable(),
  created_at_ms: U64,
  started_at_ms: U64.nullable(),
  ended_at_ms: U64.nullable(),
  last_seq: U64,
})
export const AgentsSchema = z.strictObject({
  agents: z.array(AgentSchema).max(64).readonly(),
  more_agents: z.boolean(),
  tasks: z.array(AgentTaskSchema).max(64).readonly(),
  more_tasks: z.boolean(),
})

export type HostedAgent = z.infer<typeof AgentSchema>
export type HostedAgentTask = z.infer<typeof AgentTaskSchema>
export type AgentsView = z.infer<typeof AgentsSchema>
