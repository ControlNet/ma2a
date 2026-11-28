import { waitFor } from "@testing-library/react"
import { expect, test, vi } from "vitest"

import {
  createRuntimeApiClient,
  RUNTIME_BOOT_ID_HEADER,
  RuntimeApiPayloadError,
} from "./api/client"
import {
  DEFAULT_BOOT_ID,
  runtimeSnapshot as rawSnapshot,
  stampedRuntimeSnapshot as runtimeSnapshot,
} from "./api/test-fixtures"
import { RuntimeController, UNCERTAIN_GRACE_MS } from "./runtime-controller"
import { fakeClient, flushRecovery, ManualScheduler } from "./test/runtime-controller-fakes"

test("closes the event stream and keeps backoff when an invalidation refresh fails", async () => {
  const scheduler = new ManualScheduler()
  let reads = 0
  const snapshots = [
    Promise.resolve(runtimeSnapshot(10)),
    Promise.reject(new TypeError("down")),
    Promise.resolve(runtimeSnapshot(11)),
  ]
  const given = fakeClient(() => {
    reads += 1
    return snapshots.shift() ?? Promise.reject(new TypeError("unexpected"))
  })
  const controller = new RuntimeController(
    { onRuntime: () => undefined, onSessionExpired: () => undefined, onError: () => undefined },
    given.client,
    scheduler,
  )
  await controller.start()
  given.callbacks[0]?.onOpen()

  given.callbacks[0]?.onEvent({ type: "snapshot_invalidated", revision: 11, changed: {} })
  await flushRecovery()
  expect(given.closed()).toBe(1)

  given.callbacks[0]?.onEvent({ type: "snapshot_invalidated", revision: 12, changed: {} })
  await flushRecovery()
  expect(reads).toBe(2)
  expect(scheduler.pending()).toBe(1)
  controller.stop()
})

test("ignores late callbacks from a closed subscription while in offline backoff", async () => {
  const scheduler = new ManualScheduler()
  let reads = 0
  const errors: unknown[] = []
  const snapshots = [
    Promise.resolve(runtimeSnapshot(10)),
    Promise.reject(new TypeError("down-1")),
    Promise.reject(new TypeError("down-2")),
  ]
  const given = fakeClient(() => {
    reads += 1
    return snapshots.shift() ?? Promise.reject(new TypeError("unexpected"))
  })
  const controller = new RuntimeController(
    {
      onRuntime: () => undefined,
      onSessionExpired: () => undefined,
      onError: (error) => errors.push(error),
    },
    given.client,
    scheduler,
  )
  await controller.start()
  given.callbacks[0]?.onOpen()

  given.callbacks[0]?.onEvent({ type: "snapshot_invalidated", revision: 11, changed: {} })
  await flushRecovery()
  expect(given.closed()).toBe(1)
  expect(reads).toBe(2)
  expect(scheduler.pending()).toBe(1)

  given.callbacks[0]?.onOpen()
  given.callbacks[0]?.onResyncRequired()
  given.callbacks[0]?.onDisconnect()
  given.callbacks[0]?.onPayloadError(new RuntimeApiPayloadError(["late"]))
  await flushRecovery()
  expect(reads).toBe(2)

  scheduler.runNext()
  await flushRecovery()
  expect(reads).toBe(3)
  expect(scheduler.delays.filter((delay) => delay !== UNCERTAIN_GRACE_MS)).toEqual([250, 1_000])
  expect(errors).toHaveLength(2)
  controller.stop()
})

test("recovers from a stalled snapshot body so the next refresh can run", async () => {
  vi.useFakeTimers()
  try {
    let calls = 0
    const stalled = new ReadableStream<Uint8Array>({ start() {} })
    vi.spyOn(globalThis, "fetch").mockImplementation(async () => {
      calls += 1
      if (calls === 2) {
        return new Response(stalled, {
          status: 200,
          headers: {
            "content-type": "application/x-ndjson",
            [RUNTIME_BOOT_ID_HEADER]: DEFAULT_BOOT_ID,
          },
        })
      }
      return Response.json(rawSnapshot(calls === 1 ? 10 : 11), {
        headers: { [RUNTIME_BOOT_ID_HEADER]: DEFAULT_BOOT_ID },
      })
    })
    const scheduler = new ManualScheduler()
    const connections: (string | undefined)[] = []
    const client = createRuntimeApiClient(
      { snapshot: "/api/v1/snapshot", events: "/api/v1/events" },
      () => ({ addEventListener: () => undefined, close: () => undefined }),
    )
    const controller = new RuntimeController(
      {
        onRuntime: (runtime) => connections.push(runtime?.connection),
        onSessionExpired: () => undefined,
        onError: () => undefined,
      },
      client,
      scheduler,
    )
    await controller.start()

    let settled = false
    const refreshing = controller.refresh().finally(() => {
      settled = true
    })
    for (let i = 0; i < 30; i += 1) await Promise.resolve()
    vi.advanceTimersByTime(30_000)
    for (let i = 0; i < 30; i += 1) await Promise.resolve()

    expect(settled).toBe(true)
    await refreshing
    expect(stalled.locked).toBe(false)
    expect(connections.at(-1)).toBe("offline")
    expect(scheduler.pending()).toBe(1)

    scheduler.runNext()
    vi.useRealTimers()
    await waitFor(() => expect(connections.at(-1)).toBe("online"))
    expect(calls).toBe(3)
    controller.stop()
  } finally {
    vi.useRealTimers()
    vi.restoreAllMocks()
  }
})
