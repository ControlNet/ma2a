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
}
const OPERATIONS = { active_count: 0, active: [], recent_count: 0, recent: [] }

test("accepts the Runtime's exposure and operation payloads", () => {
  const parsed = parseServicesData([EXPOSURE], OPERATIONS)

  expect(parsed.exposures).toEqual([EXPOSURE])
  expect(parsed.operations.recent_count).toBe(0)
})

test.each([
  ["an unknown field", [{ ...EXPOSURE, authorized_via: "ab".repeat(32) }], OPERATIONS],
  ["an unknown service kind", [{ ...EXPOSURE, kind: "ftp" }], OPERATIONS],
  ["a missing nullable field", [{ ...EXPOSURE, host: undefined }], OPERATIONS],
  ["a malformed operation", [], { ...OPERATIONS, active: [{ request_id: "00" }] }],
])("rejects %s", (_, exposures, operations) => {
  expect(() => parseServicesData(exposures, operations)).toThrow(RuntimeApiPayloadError)
})
