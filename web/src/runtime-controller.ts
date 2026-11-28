import { HTTPError } from "ky"
import {
  createRuntimeApiClient,
  type RuntimeApiClient,
  RuntimeStateCoordinator,
  type StampedRuntimeSnapshot,
} from "./api/client"
import type { SpaceDetails } from "./api/codec"
import {
  browserRetryScheduler,
  detailMatchesSpace,
  isSnapshotAtLeast,
  type RuntimeRetryScheduler,
  retryDelayMs,
} from "./runtime-controller-support"
import { runtimeViewFromSnapshot } from "./runtime-view"
import type { ConnectionState, RuntimeViewData } from "./view-model"

export type RuntimeControllerCallbacks = {
  readonly onRuntime: (runtime: RuntimeViewData | undefined) => void
  readonly onSessionExpired: () => void
  readonly onError: (error: unknown) => void
}

export type { RuntimeRetryScheduler }

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

export class RuntimeController {
  private readonly coordinator: RuntimeStateCoordinator
  private closeEvents: (() => void) | undefined
  private latest: StampedRuntimeSnapshot | undefined
  private cancelRetry: (() => void) | undefined
  private retryAttempt = 0
  private recoveryPending = false
  private refreshQueued = false
  private offline = false
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
      onSnapshot: (stamped) => this.install(stamped),
      onRecoveryError: (error) => this.handleError(error),
    })
  }

  async start(): Promise<void> {
    if (this.stopped || this.recoveryPending) return
    this.recoveryPending = true
    try {
      const stamped = await this.client.fetchSnapshot()
      if (this.stopped) return
      if (isSnapshotAtLeast(stamped, this.latest)) {
        this.coordinator.install(stamped)
      }
    } catch (error) {
      this.handleError(error)
      this.scheduleRetry()
    } finally {
      this.recoveryPending = false
      if (this.refreshQueued && !this.stopped) {
        this.refreshQueued = false
        void this.refresh()
      }
    }
  }

  async refresh(): Promise<void> {
    if (this.stopped) return
    if (this.recoveryPending) {
      this.refreshQueued = true
      return
    }
    this.recoveryPending = true
    this.cancelRetry?.()
    this.cancelRetry = undefined
    const settled = this.scheduler.schedule(UNCERTAIN_GRACE_MS, () => {
      if (!this.stopped) this.emit("uncertain")
    })
    try {
      await this.coordinator.resyncRequired()
    } finally {
      settled()
      this.recoveryPending = false
    }
    if (this.stopped) return
    if (this.coordinator.currentState().kind === "uncertain") {
      this.offline = true
      this.refreshQueued = false
      this.dropEvents()
      this.emit("offline")
      this.scheduleRetry()
    } else if (this.refreshQueued) {
      this.refreshQueued = false
      await this.refresh()
    }
  }

  stop(): void {
    this.stopped = true
    this.clearDetails()
    this.cancelRetry?.()
    this.cancelRetry = undefined
    this.dropEvents()
  }

  private dropEvents(): void {
    this.closeEvents?.()
    this.closeEvents = undefined
  }

  private emit(connection: ConnectionState): void {
    if (this.latest === undefined) return
    this.callbacks.onRuntime(
      runtimeViewFromSnapshot(this.latest.snapshot, connection, this.details, this.failedDetails),
    )
  }

  private install(stamped: StampedRuntimeSnapshot): void {
    if (this.stopped) return
    if (this.latest !== undefined && this.latest.bootId !== stamped.bootId) this.clearDetails()
    const { snapshot } = stamped
    this.latest = stamped
    for (const [id, detail] of this.details) {
      if (
        !snapshot.spaces.some(
          (space) => space.space_id === id && space.chain_hash === detail.space.chain_hash,
        )
      )
        this.details.delete(id)
    }
    this.failedDetails.clear()
    this.offline = false
    this.cancelRetry?.()
    this.cancelRetry = undefined
    this.emit("online")
    this.loadDetails()
    this.dropEvents()
    // A stream the Runtime refuses at once, as when every event connection is taken, would
    // otherwise reload the snapshot in a tight loop. Only an open stream resets the pause, and
    // only losing one reconnects at once. A refused stream missed no event, so the details
    // being loaded for this snapshot still stand.
    let opened = false
    this.closeEvents = this.client.subscribe(
      snapshot.revision,
      stamped.bootId,
      {
        onOpen: () => {
          opened = true
          this.retryAttempt = 0
        },
        onEvent: () => {
          if (!this.offline) void this.refresh()
        },
        onResyncRequired: () => {
          this.clearDetails()
          void this.refresh()
        },
        onDisconnect: () => {
          if (opened) {
            this.clearDetails()
            void this.refresh()
          } else this.scheduleRetry()
        },
        onPayloadError: (error) => this.handleError(error),
      },
      snapshot.view_sequence,
    )
  }

  private clearDetails(): void {
    this.detailEpoch += 1
    this.details.clear()
    this.failedDetails.clear()
  }

  private publishDetails(): void {
    if (!this.stopped && !this.offline) {
      this.emit(this.coordinator.currentState().kind === "uncertain" ? "uncertain" : "online")
    }
  }

  private loadDetails(): void {
    const latestSnapshot = this.latest?.snapshot
    if (this.stopped || latestSnapshot === undefined) return
    for (const space of latestSnapshot.spaces) {
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
          const current = this.latest?.snapshot.spaces.find(
            (item) => item.space_id === space.space_id,
          )
          if (current === undefined || current.chain_hash !== space.chain_hash) return
          if (!detailMatchesSpace(detail, current)) {
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
          const current = this.latest?.snapshot.spaces.find(
            (item) => item.space_id === space.space_id,
          )
          if (current?.chain_hash !== space.chain_hash) return
          this.failedDetails.add(space.space_id)
          this.handleError(error)
        })
        .finally(() => {
          this.detailWorkers -= 1
          this.detailPending.delete(key)
          if (this.stopped || this.offline) return
          if (epoch === this.detailEpoch) this.publishDetails()
          this.loadDetails()
        })
    }
  }

  /**
   * Reports one failure of this controller's own requests.
   *
   * A stopped controller belongs to a page or session that has moved on: a
   * request of it that fails late must neither paint an error over the new
   * page nor end the session that replaced it.
   */
  private handleError(error: unknown): void {
    if (this.stopped) return
    if (error instanceof HTTPError && error.response.status === 401) {
      this.stop()
      this.callbacks.onSessionExpired()
      return
    }
    this.callbacks.onError(error)
  }

  private scheduleRetry(): void {
    if (this.stopped || this.cancelRetry !== undefined) return
    const delay = retryDelayMs(this.retryAttempt, this.latest !== undefined)
    if (delay === undefined) return
    this.retryAttempt += 1
    this.cancelRetry = this.scheduler.schedule(delay, () => {
      this.cancelRetry = undefined
      void this.refresh()
    })
  }
}
