import ky from "ky"
import { afterEach, expect, test, vi } from "vitest"

import { createRuntimeMutationClient, mutationOutcomeUnknown } from "./mutations"

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
  const create = vi.spyOn(ky, "create")
  const client = createRuntimeMutationClient({
    csrfToken: () => "cd".repeat(32),
    requestId: () => "01".repeat(16),
  })

  const result = await client.createSpace("Operations")

  expect(result).toEqual({ space_id: "ab".repeat(32), name: "Operations", member_count: 1 })
  expect(requests).toHaveLength(1)
  expect(new URL(requests[0]?.url ?? "https://invalid/").pathname).toBe("/api/v1/spaces/create")
  // Checked as configured: Bun's Request ignores the option and reports "include".
  expect(create.mock.calls[0]?.[0]?.credentials).toBe("same-origin")
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
    csrfToken: () => "cd".repeat(32),
    requestId: () => "01".repeat(16),
  })

  const result = client.createSpace("Operations")

  await expect(result).rejects.toMatchObject({ name: "RuntimeMutationPayloadError" })
})

test("surfaces typed Local API rejection code and remediation instead of payload error", async () => {
  vi.spyOn(globalThis, "fetch").mockResolvedValue(
    Response.json({
      version: 1,
      error: "unauthorized",
      remediation: "Space owner cannot be removed in Phase 1",
    }),
  )
  const client = createRuntimeMutationClient({
    csrfToken: () => "cd".repeat(32),
    requestId: () => "01".repeat(16),
  })

  await expect(client.revokeEndpoint("ab".repeat(32), "ef".repeat(32))).rejects.toMatchObject({
    name: "RuntimeMutationRejectedError",
    code: "unauthorized",
    remediation: "Space owner cannot be removed in Phase 1",
  })
})

test("accepts nullable remediation on Local API rejection envelopes", async () => {
  vi.spyOn(globalThis, "fetch").mockResolvedValue(
    Response.json({
      version: 1,
      error: "conflict",
      remediation: null,
    }),
  )
  const client = createRuntimeMutationClient({
    csrfToken: () => "cd".repeat(32),
    requestId: () => "01".repeat(16),
  })

  await expect(client.createSpace("Operations")).rejects.toMatchObject({
    name: "RuntimeMutationRejectedError",
    code: "conflict",
    remediation: undefined,
  })
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
    csrfToken: () => "cd".repeat(32),
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
    csrfToken: () => "cd".repeat(32),
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

test("reads the CSRF token for every mutation, so one replaced by another tab is used", async () => {
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
  let token = "cd".repeat(32)
  const client = createRuntimeMutationClient({
    csrfToken: () => token,
    requestId: () => "01".repeat(16),
  })

  await client.createSpace("Operations")
  token = "ef".repeat(32)
  await client.createSpace("Operations")

  expect(requests.map((request) => request.headers.get("x-csrf-token"))).toEqual([
    "cd".repeat(32),
    "ef".repeat(32),
  ])
})

// A creation whose answer was lost may have committed, so a retry of the same name must replay
// it under its request ID instead of creating a second Space. On a retry, only an answer from the
// Runtime for that ID settles it: an HTTP refusal or timeout says nothing about the earlier
// attempt, and `unavailable` is the Runtime still running it. A fresh attempt refused before
// dispatch (such as HTTP 403), a settled creation, or another name gets a new ID.
test("a Space creation retried after a lost answer keeps its request ID", async () => {
  const ids: unknown[] = []
  const answers: (number | string)[] = []
  vi.spyOn(globalThis, "fetch").mockImplementation(async (input, init): Promise<Response> => {
    const command = (await new Request(input, init).json()) as { request_id: unknown }
    ids.push(command.request_id)
    const answer = answers.shift()
    if (answer === undefined) throw new TypeError("Failed to fetch")
    if (typeof answer === "number") return new Response(null, { status: answer })
    if (answer !== "create") return Response.json({ version: 1, error: answer })
    return Response.json({
      version: 1,
      request_id: command.request_id,
      revision: 12,
      result: {
        type: "space_created",
        payload: { space_id: "ab".repeat(32), name: "Operations", member_count: 1 },
      },
    })
  })
  let next = 0
  const client = createRuntimeMutationClient({
    csrfToken: () => "cd".repeat(32),
    requestId: () => (++next).toString(16).padStart(32, "0"),
  })

  await expect(client.createSpace("Operations")).rejects.toThrow()
  await expect(client.createSpace("Renamed")).rejects.toThrow()
  answers.push(403, 408, "unavailable", "create", 403, "invalid_input", "create")
  for (const succeeds of [false, false, false, true, false, false, true]) {
    const created = client.createSpace("Renamed")
    await (succeeds ? expect(created).resolves.toBeDefined() : expect(created).rejects.toThrow())
  }

  expect(ids).toEqual([1, 2, 2, 2, 2, 2, 3, 4, 5].map((id) => id.toString(16).padStart(32, "0")))
})

// A timeout can answer after the command reached the Runtime; any other HTTP refusal comes first.
test.each([
  [408, true],
  [403, false],
])("an HTTP %i leaves the mutation outcome unknown: %s", async (status, unknown) => {
  vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response(null, { status }))
  const client = createRuntimeMutationClient({ csrfToken: () => "cd".repeat(32) })

  const error: unknown = await client.echo("ef".repeat(32), "ping").catch((caught) => caught)

  expect(mutationOutcomeUnknown(error)).toBe(unknown)
})
