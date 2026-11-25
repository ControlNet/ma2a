import { expect, test } from "vitest"

import { RuntimeApiPayloadError } from "./codec"
import { parseServicesData } from "./services"

const EXPOSURE = {
  space_id: "ab".repeat(32),
  kind: "files_root",
  root: "docs",
  path: "/srv/docs",
  access: "read_write",
  host: null,
  port_start: null,
  port_end: null,
  agent: null,
  service: null,
}
const OPERATIONS = { active_count: 0, active: [], recent_count: 0, recent: [] }
const PROCESS = {
  process_id: "0d".repeat(16),
  creator_endpoint_id: "44".repeat(32),
  request_id: "0f".repeat(16),
  name: "ticker",
  state: "exited",
  reason: null,
  exit_code: 0,
  signal: null,
  created_at_ms: 1_000,
  started_at_ms: 1_100,
  ended_at_ms: 9_000,
  archived: false,
}
const PROCESSES = { processes: [PROCESS], next: null }
const AGENT = {
  agent: "helper",
  description: "answers questions",
  tools: ["files", "agent"],
  max_tasks: 4,
  enabled: true,
  generation: 2,
  live_tasks: 1,
  instance: {
    generation: 2,
    process_id: "0a".repeat(16),
    ready: true,
    started_at_ms: 1_000,
    ended_at_ms: null,
  },
}
const TASK = {
  task_id: "0b".repeat(16),
  agent: "helper",
  creator_endpoint_id: "44".repeat(32),
  from_agent: "lead",
  request_id: "0c".repeat(16),
  state: "running",
  reason: null,
  created_at_ms: 1_000,
  started_at_ms: 1_100,
  ended_at_ms: null,
  changed_at_ms: 1_100,
  last_seq: 3,
}
const AGENTS = { agents: [AGENT], more_agents: false, tasks: [TASK], more_tasks: true }
const HOSTED_SERVICE = {
  service: "example",
  description: "echoes",
  versions: [{ version: 1, operations: [{ name: "echo", interaction: "unary" }] }],
  max_calls: 2,
  enabled: true,
  state: "ready",
  running_calls: 0,
  calls: [{ version: 1, count: 4 }],
}
const HOSTED = { services: [HOSTED_SERVICE], more: false }

test("accepts the Runtime's exposure, operation, Process, and Agent payloads", () => {
  const parsed = parseServicesData([EXPOSURE], OPERATIONS, [PROCESSES, AGENTS, HOSTED])

  expect(parsed.exposures).toEqual([EXPOSURE])
  expect(parsed.operations.recent_count).toBe(0)
  expect(parsed.processes.processes).toEqual([PROCESS])
  expect(parsed.agents).toEqual(AGENTS)
  expect(parsed.hosted).toEqual(HOSTED)
})

test.each([
  ["an unknown field", [{ ...EXPOSURE, authorized_via: "ab".repeat(32) }], OPERATIONS, PROCESSES],
  ["an unknown service kind", [{ ...EXPOSURE, kind: "ftp" }], OPERATIONS, PROCESSES],
  ["a missing nullable field", [{ ...EXPOSURE, host: undefined }], OPERATIONS, PROCESSES],
  ["a malformed operation", [], { ...OPERATIONS, active: [{ request_id: "00" }] }, PROCESSES],
  [
    "an unknown Process state",
    [],
    OPERATIONS,
    { processes: [{ ...PROCESS, state: "paused" }], next: null },
  ],
  ["Process arguments", [], OPERATIONS, { processes: [{ ...PROCESS, args: ["-c"] }], next: null }],
])("rejects %s", (_, exposures, operations, processes) => {
  expect(() => parseServicesData(exposures, operations, [processes, AGENTS, HOSTED])).toThrow(
    RuntimeApiPayloadError,
  )
})

test.each([
  ["an Agent's program", { ...AGENTS, agents: [{ ...AGENT, program: "/bin/sh" }] }],
  ["an Agent's environment names", { ...AGENTS, agents: [{ ...AGENT, env_names: ["KEY"] }] }],
  ["an unknown tool family", { ...AGENTS, agents: [{ ...AGENT, tools: ["shell"] }] }],
  ["an unknown task state", { ...AGENTS, tasks: [{ ...TASK, state: "working" }] }],
  ["a task message", { ...AGENTS, tasks: [{ ...TASK, input: { role: "user", parts: [] } }] }],
])("rejects Agents carrying %s", (_, agents) => {
  expect(() => parseServicesData([], OPERATIONS, [PROCESSES, agents, HOSTED])).toThrow(
    RuntimeApiPayloadError,
  )
})

test.each([
  ["a call body", { ...HOSTED, services: [{ ...HOSTED_SERVICE, body: "aGk=" }] }],
  [
    "a caller",
    { ...HOSTED, services: [{ ...HOSTED_SERVICE, caller_endpoint_id: "44".repeat(32) }] },
  ],
  ["an unknown state", { ...HOSTED, services: [{ ...HOSTED_SERVICE, state: "running" }] }],
  [
    "an unknown interaction",
    {
      ...HOSTED,
      services: [
        {
          ...HOSTED_SERVICE,
          versions: [{ version: 1, operations: [{ name: "echo", interaction: "rpc" }] }],
        },
      ],
    },
  ],
])("rejects hosted services carrying %s", (_, hosted) => {
  expect(() => parseServicesData([], OPERATIONS, [PROCESSES, AGENTS, hosted])).toThrow(
    RuntimeApiPayloadError,
  )
})
