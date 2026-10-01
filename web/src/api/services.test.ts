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

test("accepts the Runtime's exposure, operation, and Process payloads", () => {
  const parsed = parseServicesData([EXPOSURE], OPERATIONS, PROCESSES)

  expect(parsed.exposures).toEqual([EXPOSURE])
  expect(parsed.operations.recent_count).toBe(0)
  expect(parsed.processes.processes).toEqual([PROCESS])
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
  expect(() => parseServicesData(exposures, operations, processes)).toThrow(RuntimeApiPayloadError)
})
