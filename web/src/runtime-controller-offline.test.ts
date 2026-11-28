import { expect, test } from "vitest"

import { stampedRuntimeSnapshot as runtimeSnapshot } from "./api/test-fixtures"
import { RuntimeController } from "./runtime-controller"
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
