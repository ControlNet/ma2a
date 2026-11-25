import { HTTPError } from "ky"
import { describe, expect, test } from "vitest"

import { stampedRuntimeSnapshot as runtimeSnapshot } from "./api/test-fixtures"
import { RuntimeController, UNCERTAIN_GRACE_MS } from "./runtime-controller"
import { fakeClient, flushRecovery, ManualScheduler } from "./test/runtime-controller-fakes"

describe("runtime controller recovery", () => {
  test("reports an initial snapshot failure and stops after three scheduled retries", async () => {
    const scheduler = new ManualScheduler()
    const failure = new TypeError("network unavailable")
    const errors: unknown[] = []
    const runtimes: unknown[] = []
    let attempts = 0
    const given = fakeClient(() => {
      attempts += 1
      return Promise.reject(failure)
    })
    const controller = new RuntimeController(
      {
        onRuntime: (runtime) => runtimes.push(runtime),
        onSessionExpired: () => undefined,
        onError: (error) => errors.push(error),
      },
      given.client,
      scheduler,
    )

    await controller.start()
    for (let retry = 0; retry < 3; retry += 1) {
      scheduler.runNext()
      await flushRecovery()
    }

    expect(attempts).toBe(4)
    expect(errors).toEqual([failure, failure, failure, failure])
    expect(runtimes).toEqual([])
    expect(scheduler.delays.filter((delay) => delay !== UNCERTAIN_GRACE_MS)).toEqual([
      250, 1_000, 4_000,
    ])
    expect(scheduler.pending()).toBe(0)
  })

  test("marks the last authoritative snapshot offline after replacement fails", async () => {
    const scheduler = new ManualScheduler()
    const snapshots = [Promise.resolve(runtimeSnapshot(10)), Promise.reject(new TypeError("down"))]
    const given = fakeClient(() => snapshots.shift() ?? Promise.reject(new TypeError("unexpected")))
    const connections: string[] = []
    const controller = new RuntimeController(
      {
        onRuntime: (runtime) => {
          if (runtime !== undefined) connections.push(runtime.connection)
        },
        onSessionExpired: () => undefined,
        onError: () => undefined,
      },
      given.client,
      scheduler,
    )
    await controller.start()

    given.callbacks[0]?.onOpen()
    given.callbacks[0]?.onDisconnect()
    await flushRecovery()

    expect(connections).toEqual(["online", "offline"])
    expect(scheduler.pending()).toBe(1)
  })

  test("recovery replaces the snapshot and owns exactly one event subscription", async () => {
    const scheduler = new ManualScheduler()
    const snapshots = [
      Promise.resolve(runtimeSnapshot(10)),
      Promise.reject(new TypeError("down")),
      Promise.resolve(runtimeSnapshot(20)),
    ]
    const given = fakeClient(() => snapshots.shift() ?? Promise.reject(new TypeError("unexpected")))
    let installed = 0
    const controller = new RuntimeController(
      {
        onRuntime: (runtime) => {
          if (runtime?.connection === "online") installed += 1
        },
        onSessionExpired: () => undefined,
        onError: () => undefined,
      },
      given.client,
      scheduler,
    )
    await controller.start()
    given.callbacks[0]?.onOpen()
    given.callbacks[0]?.onDisconnect()
    await flushRecovery()

    scheduler.runNext()
    await flushRecovery()

    expect(installed).toBe(2)
    expect(given.baselines).toEqual([10, 20])
    expect(given.callbacks).toHaveLength(2)
    expect(given.closed()).toBe(1)
    expect(scheduler.pending()).toBe(0)
  })

  test("stop cancels pending recovery and closes the event subscription", async () => {
    const scheduler = new ManualScheduler()
    const snapshots = [Promise.resolve(runtimeSnapshot(10)), Promise.reject(new TypeError("down"))]
    const given = fakeClient(() => snapshots.shift() ?? Promise.resolve(runtimeSnapshot(20)))
    const controller = new RuntimeController(
      {
        onRuntime: () => undefined,
        onSessionExpired: () => undefined,
        onError: () => undefined,
      },
      given.client,
      scheduler,
    )
    await controller.start()
    given.callbacks[0]?.onOpen()
    given.callbacks[0]?.onDisconnect()
    await flushRecovery()

    controller.stop()
    scheduler.runNext()
    await flushRecovery()

    expect(scheduler.pending()).toBe(0)
    expect(given.closed()).toBe(1)
    expect(given.callbacks).toHaveLength(1)
  })
})

test("an event stream refused at once is reopened only after a growing pause", async () => {
  // Given: a Runtime whose snapshot loads but whose event stream fails before it opens, as
  // when every event connection is taken.
  const scheduler = new ManualScheduler()
  let reads = 0
  const given = fakeClient(() => {
    reads += 1
    return Promise.resolve(runtimeSnapshot(10))
  })
  const controller = new RuntimeController(
    { onRuntime: () => undefined, onSessionExpired: () => undefined, onError: () => undefined },
    given.client,
    scheduler,
  )
  await controller.start()

  // When: each stream fails before opening.
  for (let attempt = 0; attempt < 4; attempt += 1) {
    given.callbacks.at(-1)?.onDisconnect()
    await flushRecovery()
    scheduler.runNext()
    await flushRecovery()
  }

  // Then: every reconnect waited, and the pause grew to its bound.
  expect(reads).toBe(5)
  expect(scheduler.delays.filter((delay) => delay !== UNCERTAIN_GRACE_MS)).toEqual([
    250, 1_000, 4_000, 4_000,
  ])

  // Once a stream opened, losing it reconnects at once and from the first pause again.
  given.callbacks.at(-1)?.onOpen()
  given.callbacks.at(-1)?.onDisconnect()
  await flushRecovery()
  expect(reads).toBe(6)
  given.callbacks.at(-1)?.onDisconnect()
  await flushRecovery()
  expect(scheduler.delays.filter((delay) => delay !== UNCERTAIN_GRACE_MS).at(-1)).toBe(250)
  controller.stop()
})

test("a stopped controller reports nothing when its pending request fails late", async () => {
  const request = new Request("http://localhost/api/v1/snapshot")
  const unauthorized = new HTTPError(
    new Response(null, { status: 401 }),
    request,
    {} as ConstructorParameters<typeof HTTPError>[2],
  )
  for (const failure of [unauthorized, new TypeError("network unavailable")]) {
    const scheduler = new ManualScheduler()
    let fail: (error: unknown) => void = () => undefined
    const pending = new Promise<never>((_resolve, reject) => {
      fail = reject
    })
    const given = fakeClient(() => pending)
    let expired = 0
    const errors: unknown[] = []
    const controller = new RuntimeController(
      {
        onRuntime: () => undefined,
        onSessionExpired: () => {
          expired += 1
        },
        onError: (error) => errors.push(error),
      },
      given.client,
      scheduler,
    )
    const started = controller.start()

    controller.stop()
    fail(failure)
    await started

    expect(expired).toBe(0)
    expect(errors).toEqual([])
    expect(scheduler.pending()).toBe(0)
  }
})

test("an ordinary revision change never paints the snapshot as uncertain", async () => {
  const scheduler = new ManualScheduler()
  const snapshots = [Promise.resolve(runtimeSnapshot(10)), Promise.resolve(runtimeSnapshot(11))]
  const given = fakeClient(() => snapshots.shift() ?? Promise.reject(new TypeError("unexpected")))
  const connections: (string | undefined)[] = []
  const controller = new RuntimeController(
    {
      onRuntime: (runtime) => connections.push(runtime?.connection),
      onSessionExpired: () => undefined,
      onError: () => undefined,
    },
    given.client,
    scheduler,
  )
  await controller.start()
  given.callbacks[0]?.onEvent({ type: "snapshot_invalidated", revision: 11, changed: {} })
  await flushRecovery()

  expect(connections).not.toContain("uncertain")
  expect(connections.at(-1)).toBe("online")
  expect(scheduler.pending()).toBe(0)
})
