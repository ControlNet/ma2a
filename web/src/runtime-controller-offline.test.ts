import { expect, test } from "vitest"

import { RuntimeApiPayloadError } from "./api/client"
import { stampedRuntimeSnapshot as runtimeSnapshot } from "./api/test-fixtures"
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
