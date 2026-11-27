import { afterEach, expect, test, vi } from "vitest"

import type { RequestId } from "./generated"
import { createRuntimeMutationClient } from "./mutations"
import {
  clearPendingSpaceCreation,
  createPendingSpaceTracker,
  PENDING_SPACE_STORAGE_KEY,
  type PendingSpaceCreation,
  readPendingSpaceCreation,
  writePendingSpaceCreation,
} from "./pending-space"

afterEach(() => {
  vi.restoreAllMocks()
})

function memoryStorage(): Storage {
  const entries = new Map<string, string>()
  return {
    get length() {
      return entries.size
    },
    clear: () => entries.clear(),
    getItem: (key) => entries.get(key) ?? null,
    key: (index) => Array.from(entries.keys())[index] ?? null,
    removeItem: (key) => void entries.delete(key),
    setItem: (key, value) => void entries.set(key, value),
  }
}

test("persists and scopes pending Space creation records by Runtime endpoint ID", () => {
  const storage = memoryStorage()
  const entry = {
    endpointId: "aa".repeat(32),
    name: "Operations",
    requestId: "01".repeat(16) as RequestId,
  }

  writePendingSpaceCreation(entry, storage)

  expect(readPendingSpaceCreation("aa".repeat(32), storage)).toEqual(entry)
  expect(readPendingSpaceCreation("bb".repeat(32), storage)).toBeUndefined()
  expect(readPendingSpaceCreation(undefined, storage)).toBeUndefined()

  clearPendingSpaceCreation(storage)
  expect(readPendingSpaceCreation("aa".repeat(32), storage)).toBeUndefined()
})

test("drops malformed stored pending Space creation entries safely", () => {
  const storage = memoryStorage()
  storage.setItem(PENDING_SPACE_STORAGE_KEY, "{bad-json")
  expect(readPendingSpaceCreation("aa".repeat(32), storage)).toBeUndefined()
  expect(storage.getItem(PENDING_SPACE_STORAGE_KEY)).toBeNull()

  storage.setItem(
    PENDING_SPACE_STORAGE_KEY,
    JSON.stringify({ endpointId: "aa".repeat(32), name: "Operations", requestId: "not-hex" }),
  )
  expect(readPendingSpaceCreation("aa".repeat(32), storage)).toBeUndefined()
  expect(storage.getItem(PENDING_SPACE_STORAGE_KEY)).toBeNull()
})

test("falls back to in-memory tracking and notifies onChange when storage throws or is absent", () => {
  const blocked: Storage = {
    length: 0,
    clear: () => {
      throw new Error("blocked")
    },
    getItem: () => {
      throw new Error("blocked")
    },
    key: () => null,
    removeItem: () => {
      throw new Error("blocked")
    },
    setItem: () => {
      throw new Error("blocked")
    },
  }
  for (const storage of [blocked, undefined]) {
    const observed: (PendingSpaceCreation | undefined)[] = []
    let next = 0
    const nextId = (): RequestId => (++next).toString(16).padStart(32, "0") as RequestId
    const tracker = createPendingSpaceTracker({
      endpointId: () => "aa".repeat(32),
      storage,
      onChange: (entry) => observed.push(entry),
    })

    const first = tracker.begin("Operations", nextId)
    tracker.markUnsettled(first.requestId)
    const retry = tracker.begin("Operations", nextId)

    expect(first.reused).toBe(false)
    expect(retry.reused).toBe(true)
    expect(retry.requestId).toBe(first.requestId)
    expect(observed).toEqual([
      { endpointId: "aa".repeat(32), name: "Operations", requestId: first.requestId },
    ])

    tracker.settle(first.requestId)
    expect(observed.at(-1)).toBeUndefined()
  }
})

test("different-name refusal and pre-dispatch 403/422 keep Alpha's original request ID across reload", async () => {
  for (const betaAnswer of [
    new Response(null, { status: 403 }),
    new Response(null, { status: 422 }),
    Response.json({ version: 1, error: "conflict", remediation: null }),
  ]) {
    const storage = memoryStorage()
    const observed: (PendingSpaceCreation | undefined)[] = []
    const sentIds: string[] = []
    let next = 0
    const endpointId = "aa".repeat(32)
    const answers: Array<"drop" | Response | "create"> = ["drop", betaAnswer, "create"]
    vi.spyOn(globalThis, "fetch").mockImplementation(async (input, init): Promise<Response> => {
      const command = (await new Request(input, init).json()) as { request_id: string }
      sentIds.push(command.request_id)
      const answer = answers.shift()
      if (answer === "drop") throw new TypeError("Failed to fetch")
      if (answer === "create") {
        return Response.json({
          version: 1,
          request_id: command.request_id,
          revision: 12,
          result: {
            type: "space_created",
            payload: { space_id: "ab".repeat(32), name: "Alpha", member_count: 1 },
          },
        })
      }
      return answer ?? new Response(null, { status: 500 })
    })
    const makeClient = () =>
      createRuntimeMutationClient({
        csrfToken: () => "cd".repeat(32),
        requestId: () => (++next).toString(16).padStart(32, "0"),
        endpointId: () => endpointId,
        storage,
        onPendingSpaceCreation: (pending) => observed.push(pending),
      })

    const beforeReload = makeClient()
    await expect(beforeReload.createSpace("Alpha")).rejects.toThrow()
    await expect(beforeReload.createSpace("Beta")).rejects.toThrow()
    expect(readPendingSpaceCreation(endpointId, storage)).toEqual({
      endpointId,
      name: "Alpha",
      requestId: "01".padStart(32, "0") as RequestId,
    })
    expect(observed.at(-1)).toEqual({
      endpointId,
      name: "Alpha",
      requestId: "01".padStart(32, "0") as RequestId,
    })

    const afterReload = makeClient()
    await expect(afterReload.createSpace("Alpha")).resolves.toBeDefined()
    expect(sentIds).toEqual([
      "01".padStart(32, "0"),
      "02".padStart(32, "0"),
      "01".padStart(32, "0"),
    ])
    vi.restoreAllMocks()
  }
})
