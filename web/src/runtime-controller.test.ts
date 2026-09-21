import { describe, expect, test } from "vitest"

import type { RuntimeApiClient, RuntimeEventCallbacks } from "./api/client"
import { runtimeSnapshot } from "./api/test-fixtures"
import { RuntimeController, type RuntimeRetryScheduler } from "./runtime-controller"

class ManualScheduler implements RuntimeRetryScheduler {
  readonly delays: number[] = []
  private tasks: Array<(() => void) | undefined> = []

  schedule(delayMs: number, task: () => void): () => void {
    this.delays.push(delayMs)
    const index = this.tasks.push(task) - 1
    return () => {
      this.tasks[index] = undefined
    }
  }

  runNext(): void {
    const index = this.tasks.findIndex((task) => task !== undefined)
    const task = index === -1 ? undefined : this.tasks[index]
    if (task === undefined) return
    this.tasks[index] = undefined
    task()
  }

  pending(): number {
    return this.tasks.filter((task) => task !== undefined).length
  }
}

function fakeClient(fetchSnapshot: RuntimeApiClient["fetchSnapshot"]): {
  readonly client: RuntimeApiClient
  readonly callbacks: RuntimeEventCallbacks[]
  readonly closed: () => number
} {
  const callbacks: RuntimeEventCallbacks[] = []
  let closed = 0
  return {
    client: {
      fetchSnapshot,
      fetchSpaceDetails: () => Promise.reject(new Error("No Space details in this fixture")),
      subscribe: (_revision, eventCallbacks) => {
        callbacks.push(eventCallbacks)
        let active = true
        return () => {
          if (!active) return
          active = false
          closed += 1
        }
      },
    },
    callbacks,
    closed: () => closed,
  }
}

async function flushRecovery(): Promise<void> {
  for (let continuation = 0; continuation < 4; continuation += 1) {
    await Promise.resolve()
  }
}

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
    expect(scheduler.delays).toEqual([250, 1_000, 4_000])
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

    given.callbacks[0]?.onDisconnect()
    await flushRecovery()

    expect(connections).toEqual(["online", "uncertain", "offline"])
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
    const revisions: number[] = []
    const controller = new RuntimeController(
      {
        onRuntime: (runtime) => {
          if (runtime?.connection === "online") revisions.push(runtime.revision)
        },
        onSessionExpired: () => undefined,
        onError: () => undefined,
      },
      given.client,
      scheduler,
    )
    await controller.start()
    given.callbacks[0]?.onDisconnect()
    await flushRecovery()

    scheduler.runNext()
    await flushRecovery()

    expect(revisions).toEqual([10, 20])
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
