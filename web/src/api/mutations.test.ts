import { afterEach, expect, test, vi } from "vitest"

import { createRuntimeMutationClient } from "./mutations"

afterEach(() => {
  vi.restoreAllMocks()
})

test("posts a typed Space creation with same-origin CSRF and returns its result", async () => {
  const requests: Request[] = []
  vi.spyOn(globalThis, "fetch").mockImplementation(async (input, init): Promise<Response> => {
    requests.push(new Request(input, init))
    return Response.json({
      version: 1,
      request_id: "01".repeat(16),
      revision: 12,
      result: {
        type: "space_created",
        payload: { space_id: "ab".repeat(32), name: "Operations", member_count: 1 },
      },
    })
  })
  const client = createRuntimeMutationClient({
    csrfToken: "cd".repeat(32),
    requestId: () => "01".repeat(16),
  })

  const result = await client.createSpace("Operations")

  expect(result).toEqual({ space_id: "ab".repeat(32), name: "Operations", member_count: 1 })
  expect(requests).toHaveLength(1)
  expect(new URL(requests[0]?.url ?? "https://invalid/").pathname).toBe("/api/v1/spaces/create")
  expect(requests[0]?.credentials).toBe("include")
  expect(requests[0]?.headers.get("x-csrf-token")).toBe("cd".repeat(32))
  await expect(requests[0]?.json()).resolves.toEqual({
    version: 1,
    operation: "space_create",
    request_id: "01".repeat(16),
    name: "Operations",
  })
})

test("rejects a mutation response whose result does not match the command", async () => {
  vi.spyOn(globalThis, "fetch").mockResolvedValue(
    Response.json({
      version: 1,
      request_id: "01".repeat(16),
      revision: 12,
      result: { type: "shutting_down", payload: {} },
    }),
  )
  const client = createRuntimeMutationClient({
    csrfToken: "cd".repeat(32),
    requestId: () => "01".repeat(16),
  })

  const result = client.createSpace("Operations")

  await expect(result).rejects.toMatchObject({ name: "RuntimeMutationPayloadError" })
})
