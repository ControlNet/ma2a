import { HTTPError } from "ky"
import {
  createRuntimeApiClient,
  type RuntimeApiClient,
  type RuntimeSnapshot,
  RuntimeStateCoordinator,
} from "./api/client"
import type { SpaceDetails } from "./api/codec"
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

/**
 * How long a resync may run before the console reports the state as uncertain.
 *
 * A snapshot invalidation makes local state genuinely uncertain until the
 * replacement snapshot lands, and that is what the coordinator tracks. Painting
 * it immediately would flag every ordinary revision change, because the refetch
 * that resolves it usually completes in milliseconds, so the reader sees a blink
 * rather than a fact. The grace period reports only the uncertainty that lasts
 * long enough to act on; a slow or failing resync still surfaces.
 */
export const UNCERTAIN_GRACE_MS = 400

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
  private readonly details = new Map<string, SpaceDetails>()
  private readonly failedDetails = new Set<string>()
  private detailEpoch = 0
  private detailWorkers = 0
  private readonly detailPending = new Set<string>()

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
    const settled = this.scheduler.schedule(UNCERTAIN_GRACE_MS, () => {
      if (this.stopped || this.latestSnapshot === undefined) return
      this.callbacks.onRuntime(
        runtimeViewFromSnapshot(this.latestSnapshot, "uncertain", this.details, this.failedDetails),
      )
    })
    await this.coordinator.resyncRequired()
    settled()
    this.recoveryPending = false
    if (this.stopped) return
    if (this.coordinator.currentState().kind === "uncertain") {
      if (this.latestSnapshot !== undefined) {
        this.callbacks.onRuntime(
          runtimeViewFromSnapshot(this.latestSnapshot, "offline", this.details, this.failedDetails),
        )
      }
      this.scheduleRetry()
    }
  }

  stop(): void {
    this.stopped = true
    this.clearDetails()
    this.cancelRetry?.()
    this.cancelRetry = undefined
    this.closeEvents?.()
    this.closeEvents = undefined
  }

  private install(snapshot: RuntimeSnapshot): void {
    if (this.stopped) return
    this.latestSnapshot = snapshot
    for (const [id, detail] of this.details) {
      if (
        !snapshot.spaces.some(
          (space) => space.space_id === id && space.chain_hash === detail.space.chain_hash,
        )
      )
        this.details.delete(id)
    }
    this.failedDetails.clear()
    this.retryAttempt = 0
    this.recoveryPending = false
    this.cancelRetry?.()
    this.cancelRetry = undefined
    this.callbacks.onRuntime(
      runtimeViewFromSnapshot(snapshot, "online", this.details, this.failedDetails),
    )
    this.loadDetails()
    this.closeEvents?.()
    this.closeEvents = this.client.subscribe(snapshot.revision, {
      onEvent: () => void this.refresh(),
      onResyncRequired: () => {
        this.clearDetails()
        void this.refresh()
      },
      onDisconnect: () => {
        this.clearDetails()
        void this.refresh()
      },
      onPayloadError: (error) => this.callbacks.onError(error),
    })
  }

  private clearDetails(): void {
    this.detailEpoch += 1
    this.details.clear()
    this.failedDetails.clear()
  }

  private publishDetails(): void {
    if (!this.stopped && this.latestSnapshot !== undefined) {
      const connection =
        this.coordinator.currentState().kind === "uncertain" ? "uncertain" : "online"
      this.callbacks.onRuntime(
        runtimeViewFromSnapshot(this.latestSnapshot, connection, this.details, this.failedDetails),
      )
    }
  }

  private loadDetails(): void {
    if (this.stopped || this.latestSnapshot === undefined) return
    for (const space of this.latestSnapshot.spaces) {
      if (this.detailWorkers >= 4) break
      const key = `${this.detailEpoch}:${space.space_id}:${space.chain_hash}`
      if (
        this.details.has(space.space_id) ||
        this.failedDetails.has(space.space_id) ||
        this.detailPending.has(key)
      )
        continue
      const epoch = this.detailEpoch
      this.detailWorkers += 1
      this.detailPending.add(key)
      void this.client
        .fetchSpaceDetails(space.space_id)
        .then((detail) => {
          if (this.stopped || epoch !== this.detailEpoch) return
          const current = this.latestSnapshot?.spaces.find(
            (item) => item.space_id === space.space_id,
          )
          if (current === undefined || current.chain_hash !== space.chain_hash) return
          if (
            detail.space.space_id !== space.space_id ||
            detail.space.chain_hash !== current.chain_hash ||
            detail.space.member_count !== current.member_count ||
            detail.space.generation !== current.generation
          ) {
            this.failedDetails.add(space.space_id)
            this.callbacks.onError(
              new Error("Space membership changed while loading; refresh to retry."),
            )
            return
          }
          this.details.set(space.space_id, detail)
        })
        .catch((error: unknown) => {
          if (this.stopped || epoch !== this.detailEpoch) return
          const current = this.latestSnapshot?.spaces.find(
            (item) => item.space_id === space.space_id,
          )
          if (current?.chain_hash !== space.chain_hash) return
          this.failedDetails.add(space.space_id)
          this.handleError(error)
        })
        .finally(() => {
          this.detailWorkers -= 1
          this.detailPending.delete(key)
          this.publishDetails()
          this.loadDetails()
        })
    }
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
