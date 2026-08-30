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

export class RuntimeController {
  private readonly coordinator: RuntimeStateCoordinator
  private closeEvents: (() => void) | undefined
  private latestSnapshot: RuntimeSnapshot | undefined
  private stopped = false

  constructor(
    private readonly callbacks: RuntimeControllerCallbacks,
    private readonly client: RuntimeApiClient = createRuntimeApiClient({
      snapshot: "/api/v1/snapshot",
      events: "/api/v1/events",
    }),
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
    }
  }

  async refresh(): Promise<void> {
    if (this.latestSnapshot !== undefined) {
      this.callbacks.onRuntime(runtimeViewFromSnapshot(this.latestSnapshot, "uncertain"))
    }
    await this.coordinator.resyncRequired()
  }

  stop(): void {
    this.stopped = true
    this.closeEvents?.()
    this.closeEvents = undefined
  }

  private install(snapshot: RuntimeSnapshot): void {
    if (this.stopped) return
    this.latestSnapshot = snapshot
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
}
