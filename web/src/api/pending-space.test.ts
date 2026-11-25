import { expect, test } from "vitest"

import type { RequestId } from "./generated"
import {
  clearPendingSpaceCreation,
  createPendingSpaceTracker,
  PENDING_SPACE_STORAGE_KEY,
  type PendingSpaceCreation,
  readPendingSpaceCreation,
  writePendingSpaceCreation,
} from "./pending-space"

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
