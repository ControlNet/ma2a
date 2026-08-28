import { createHash } from "node:crypto"

import {
  ERROR_CODES,
  LOCAL_API_SCHEMA_JSON,
  LOCAL_API_SCHEMA_SHA256,
  LOCAL_API_VERSION,
} from "./generated"

test("generated local API contract matches the independent golden hash", () => {
  const digest = createHash("sha256").update(LOCAL_API_SCHEMA_JSON).digest("hex")

  expect(LOCAL_API_VERSION).toBe(1)
  expect(ERROR_CODES).toHaveLength(9)
  expect(digest).toBe(LOCAL_API_SCHEMA_SHA256)
  expect(LOCAL_API_SCHEMA_JSON).not.toContain("authorized_via")
})
