import { vi } from "vitest"

import type { RuntimeApiClient, RuntimeEventCallbacks } from "../api/client"
import {
  RuntimeController,
  type RuntimeControllerCallbacks,
  type RuntimeRetryScheduler,
} from "../runtime-controller"

// Each controller the console starts is held here instead of loading anything; the test plays
// the Runtime through its callbacks. A test file empties it after each test.
export const controllers: RuntimeControllerCallbacks[] = []
export function holdControllers(): void {
  vi.spyOn(RuntimeController.prototype, "start").mockImplementation(function (
    this: RuntimeController,
  ) {
    controllers.push((this as unknown as { callbacks: RuntimeControllerCallbacks }).callbacks)
    return Promise.resolve()
  })
  vi.spyOn(RuntimeController.prototype, "refresh").mockResolvedValue(undefined)
}

/** Runs each scheduled retry only when a test asks, and records every delay asked for. */
export class ManualScheduler implements RuntimeRetryScheduler {
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

export function fakeClient(fetchSnapshot: RuntimeApiClient["fetchSnapshot"]): {
  readonly client: RuntimeApiClient
  readonly callbacks: RuntimeEventCallbacks[]
  readonly baselines: number[]
  readonly closed: () => number
} {
  const callbacks: RuntimeEventCallbacks[] = []
  const baselines: number[] = []
  let closed = 0
  return {
    client: {
      fetchSnapshot,
      fetchSpaceDetails: () => Promise.reject(new Error("No Space details in this fixture")),
      subscribe: (revision, eventCallbacks) => {
        baselines.push(revision)
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
    baselines,
    closed: () => closed,
  }
}

export async function flushRecovery(): Promise<void> {
  for (let continuation = 0; continuation < 4; continuation += 1) {
    await Promise.resolve()
  }
}
