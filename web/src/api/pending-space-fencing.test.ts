import { expect, test } from "vitest"

import type { RequestId } from "./generated"
import { createPendingSpaceTracker, readPendingSpaceCreation } from "./pending-space"

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

test("fences pending-space writes and notifications to the originating session and Endpoint", () => {
  const storage = memoryStorage()
  const changes: unknown[] = []
  let activeSession = true
  let currentEndpoint: string | undefined = "aa".repeat(32)
  let next = 0
  const nextId = (): RequestId => (++next).toString(16).padStart(32, "0") as RequestId
  const tracker = createPendingSpaceTracker({
    endpointId: () => currentEndpoint,
    storage,
    isActive: () => activeSession,
    onChange: (pending) => changes.push(pending),
  })

  const alpha = tracker.begin("Alpha", nextId)
  // Seed a newer Endpoint's pending record in the same tab storage.
  currentEndpoint = "bb".repeat(32)
  const beta = tracker.begin("Beta", nextId)
  tracker.markUnsettled(beta.requestId)

  // Late Alpha settlement after switching Endpoint must not touch Beta's storage or UI.
  tracker.settle(alpha.requestId)
  expect(readPendingSpaceCreation("bb".repeat(32), storage)?.requestId).toBe(beta.requestId)
  expect(changes).toHaveLength(1)

  // Ending the originating session fences even on the same Endpoint.
  activeSession = false
  tracker.settle(beta.requestId)
  expect(readPendingSpaceCreation("bb".repeat(32), storage)?.requestId).toBe(beta.requestId)
  expect(changes).toHaveLength(1)
})
