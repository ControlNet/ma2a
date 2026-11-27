import { expect, test } from "vitest"

import { DEFAULT_BOOT_ID, stampedRuntimeSnapshot as runtimeSnapshot } from "./api/test-fixtures"
import { RuntimeController } from "./runtime-controller"
import { fakeClient, flushRecovery, ManualScheduler } from "./test/runtime-controller-fakes"

test("subscribes with view_sequence and resnapshots when view_sequence advances at the same revision", async () => {
  const scheduler = new ManualScheduler()
  const snapshots = [
    Promise.resolve(runtimeSnapshot(10, DEFAULT_BOOT_ID, 1)),
    Promise.resolve(runtimeSnapshot(10, DEFAULT_BOOT_ID, 2)),
  ]
  const given = fakeClient(() => snapshots.shift() ?? Promise.reject(new TypeError("unexpected")))
  const viewSequences: number[] = []
  const controller = new RuntimeController(
    {
      onRuntime: (runtime) => {
        if (runtime !== undefined) viewSequences.push(runtime.controlRounds.length)
      },
      onSessionExpired: () => undefined,
      onError: () => undefined,
    },
    given.client,
    scheduler,
  )
  await controller.start()

  given.callbacks[0]?.onEvent({ type: "snapshot_invalidated", revision: 10, changed: {} })
  await flushRecovery()

  expect(given.baselines).toEqual([10, 10])
  expect(given.viewSequences).toEqual([1, 2])
  expect(viewSequences).toEqual([0, 0])
  controller.stop()
})
