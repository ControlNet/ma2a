import { HTTPError } from "ky"

import {
  createRuntimeApiClient,
  type RuntimeApiClient,
  type RuntimeSnapshot,
  RuntimeStateCoordinator,
} from "./api/client"
import { runtimeViewFromSnapshot } from "./runtime-view"
import type { RuntimeViewData } from "./view-model"

export type RuntimeControllerCallbacks = {
  readonly onRuntime: (runtime: RuntimeViewData | undefined) => void
  readonly onSessionExpired: () => void
  readonly onError: (error: unknown) => void
}

export type RuntimeRetryScheduler = {
  readonly schedule: (delayMs: number, task: () => void) => () => void
}

const RETRY_DELAYS_MS = [250, 1_000, 4_000] as const

const browserRetryScheduler: RuntimeRetryScheduler = {
  schedule: (delayMs, task) => {
    const timeout = window.setTimeout(task, delayMs)
    return () => window.clearTimeout(timeout)
  },
}

export class RuntimeController {
  private readonly coordinator: RuntimeStateCoordinator
  private closeEvents: (() => void) | undefined
  private latestSnapshot: RuntimeSnapshot | undefined
  private cancelRetry: (() => void) | undefined
  private retryAttempt = 0
  private recoveryPending = false
  private stopped = false

  constructor(
    private readonly callbacks: RuntimeControllerCallbacks,
    private readonly client: RuntimeApiClient = createRuntimeApiClient({
      snapshot: "/api/v1/snapshot",
      events: "/api/v1/events",
    }),
    private readonly scheduler: RuntimeRetryScheduler = browserRetryScheduler,
  ) {
    this.coordinator = new RuntimeStateCoordinator({
      fetchSnapshot: () => this.client.fetchSnapshot(),
      onIncremental: () => undefined,
      onSnapshot: (snapshot) => this.install(snapshot),
      onRecoveryError: (error) => this.handleError(error),
    })
  }

  async start(): Promise<void> {
    try {
      this.coordinator.install(await this.client.fetchSnapshot())
    } catch (error) {
      this.handleError(error)
      this.scheduleRetry()
    }
  }

  async refresh(): Promise<void> {
    if (this.stopped || this.recoveryPending) return
    this.recoveryPending = true
    this.cancelRetry?.()
    this.cancelRetry = undefined
    if (this.latestSnapshot !== undefined) {
      this.callbacks.onRuntime(runtimeViewFromSnapshot(this.latestSnapshot, "uncertain"))
    }
    await this.coordinator.resyncRequired()
    this.recoveryPending = false
    if (this.stopped) return
    if (this.coordinator.currentState().kind === "uncertain") {
      if (this.latestSnapshot !== undefined) {
        this.callbacks.onRuntime(runtimeViewFromSnapshot(this.latestSnapshot, "offline"))
      }
      this.scheduleRetry()
    }
  }

  stop(): void {
    this.stopped = true
    this.cancelRetry?.()
    this.cancelRetry = undefined
    this.closeEvents?.()
    this.closeEvents = undefined
  }

  private install(snapshot: RuntimeSnapshot): void {
    if (this.stopped) return
    this.latestSnapshot = snapshot
    this.retryAttempt = 0
    this.recoveryPending = false
    this.cancelRetry?.()
    this.cancelRetry = undefined
    this.callbacks.onRuntime(runtimeViewFromSnapshot(snapshot, "online"))
    this.closeEvents?.()
    this.closeEvents = this.client.subscribe(snapshot.revision, {
      onEvent: () => void this.refresh(),
      onResyncRequired: () => void this.refresh(),
      onDisconnect: () => void this.refresh(),
      onPayloadError: (error) => this.callbacks.onError(error),
    })
  }

  private handleError(error: unknown): void {
    if (error instanceof HTTPError && error.response.status === 401) {
      this.stop()
      this.callbacks.onSessionExpired()
      return
    }
    this.callbacks.onError(error)
  }

  private scheduleRetry(): void {
    if (this.stopped || this.cancelRetry !== undefined) return
    const delay = RETRY_DELAYS_MS[this.retryAttempt]
    if (delay === undefined) return
    this.retryAttempt += 1
    this.cancelRetry = this.scheduler.schedule(delay, () => {
      this.cancelRetry = undefined
      void this.refresh()
    })
  }
}
