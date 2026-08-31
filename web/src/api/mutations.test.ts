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

test("posts the generated Space invitation ticket contract without browser secret material", async () => {
  const requests: Request[] = []
  vi.spyOn(globalThis, "fetch").mockImplementation(async (input, init): Promise<Response> => {
    requests.push(new Request(input, init))
    return Response.json({
      version: 1,
      request_id: "02".repeat(16),
      revision: 13,
      result: {
        type: "space_invitation_created",
        payload: { space_id: "ab".repeat(32), name: "Operations", member_count: 1 },
      },
    })
  })
  const client = createRuntimeMutationClient({
    csrfToken: "cd".repeat(32),
    requestId: () => "02".repeat(16),
  })

  await client.createInvitation("ab".repeat(32), 300_000, "/tmp/operations.invite")

  await expect(requests[0]?.json()).resolves.toEqual({
    version: 1,
    operation: "space_invite",
    request_id: "02".repeat(16),
    space_id: "ab".repeat(32),
    ttl_ms: 300_000,
    output_path: "/tmp/operations.invite",
  })
})

test("posts the complete generated Private Relay configuration contract", async () => {
  const requests: Request[] = []
  vi.spyOn(globalThis, "fetch").mockImplementation(async (input, init): Promise<Response> => {
    requests.push(new Request(input, init))
    return Response.json({
      version: 1,
      request_id: "03".repeat(16),
      revision: 14,
      result: {
        type: "private_relay_configured",
        payload: {
          configured: true,
          mode: "native_tls",
          host: "127.0.0.1",
          port: 443,
          online: true,
        },
      },
    })
  })
  const client = createRuntimeMutationClient({
    csrfToken: "cd".repeat(32),
    requestId: () => "03".repeat(16),
  })

  await client.configurePrivateRelay({
    mode: "native_tls",
    listen: "127.0.0.1:443",
    publicUrl: "https://relay.example",
    servedSpaceIds: ["ab".repeat(32)],
    certificatePath: "/tmp/relay.cert.pem",
    privateKeyPath: "/tmp/relay.key.pem",
  })

  await expect(requests[0]?.json()).resolves.toEqual({
    version: 1,
    operation: "private_relay_configure",
    request_id: "03".repeat(16),
    mode: "native_tls",
    listen: "127.0.0.1:443",
    public_url: "https://relay.example",
    served_space_ids: ["ab".repeat(32)],
    certificate_path: "/tmp/relay.cert.pem",
    private_key_path: "/tmp/relay.key.pem",
  })
})
