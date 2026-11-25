import type { RuntimeEvent } from "./codec"
import type { StampedRuntimeSnapshot } from "./snapshot-stream"
import {
  beginResnapshot,
  EMPTY_RUNTIME_STATE,
  failResnapshot,
  installSnapshot,
  markDisconnected,
  markResyncRequired,
  type RuntimeState,
  receiveRevision,
} from "./state"

function assertNever(value: never): never {
  throw new TypeError(`Unexpected recovery outcome: ${JSON.stringify(value)}`)
}

export class StaleRuntimeSnapshotError extends Error {
  readonly name = "StaleRuntimeSnapshotError"

  constructor(
    readonly lastRevision: number,
    readonly snapshotRevision: number,
  ) {
    super(`Replacement snapshot revision ${snapshotRevision} is older than ${lastRevision}`)
  }
}

export type RuntimeStateCoordinatorOptions = {
  readonly fetchSnapshot: () => Promise<StampedRuntimeSnapshot>
  readonly onIncremental: (revision: number) => void
  readonly onSnapshot: (stamped: StampedRuntimeSnapshot) => void
  readonly onRecoveryError: (error: unknown) => void
}

export class RuntimeStateCoordinator {
  private state: RuntimeState = EMPTY_RUNTIME_STATE

  constructor(private readonly options: RuntimeStateCoordinatorOptions) {}

  currentState(): RuntimeState {
    return this.state
  }

  install(stamped: StampedRuntimeSnapshot): void {
    this.state = installSnapshot(stamped.snapshot.revision, stamped.bootId)
    this.options.onSnapshot(stamped)
  }

  async receiveRevision(revision: number): Promise<boolean> {
    const transition = receiveRevision(this.state, revision)
    this.state = transition.state
    if (transition.accepted) {
      this.options.onIncremental(revision)
      return true
    }
    if (transition.effect === "resnapshot") {
      await this.recover()
    }
    return false
  }

  async receiveEvent(event: RuntimeEvent): Promise<boolean> {
    switch (event.type) {
      case "snapshot_invalidated":
        await this.resyncRequired()
        return false
      case "endpoint_changed":
      case "spaces_changed":
      case "control_sync_changed":
      case "relay_candidates_changed":
      case "relay_state_changed":
      case "reachability_changed":
      case "echo_summary_changed":
      case "ui_auth_changed":
        return this.receiveRevision(event.revision)
      default:
        return assertNever(event)
    }
  }

  async disconnect(): Promise<void> {
    const transition = markDisconnected(this.state)
    this.state = transition.state
    if (transition.effect === "resnapshot") {
      await this.recover()
    }
  }

  async resyncRequired(): Promise<void> {
    const transition = markResyncRequired(this.state)
    this.state = transition.state
    if (transition.effect === "resnapshot") {
      await this.recover()
    }
  }

  private async recover(): Promise<void> {
    const lastRevision = this.state.kind === "uncertain" ? this.state.lastRevision : null
    const lastBootId = this.state.kind === "uncertain" ? this.state.lastBootId : null
    this.state = beginResnapshot(this.state)
    const outcome = await this.options.fetchSnapshot().then(
      (stamped) => ({ kind: "success", stamped }) as const,
      (error: unknown) => ({ kind: "failure", error }) as const,
    )
    switch (outcome.kind) {
      case "success":
        // Revision order is meaningful within one Runtime boot; a confirmed new boot (such as a
        // restored database after restart) resets the projection even at a lower revision.
        if (
          lastRevision !== null &&
          lastBootId !== null &&
          outcome.stamped.bootId === lastBootId &&
          outcome.stamped.snapshot.revision < lastRevision
        ) {
          this.state = failResnapshot(this.state)
          this.options.onRecoveryError(
            new StaleRuntimeSnapshotError(lastRevision, outcome.stamped.snapshot.revision),
          )
          return
        }
        this.install(outcome.stamped)
        return
      case "failure":
        this.state = failResnapshot(this.state)
        this.options.onRecoveryError(outcome.error)
        return
      default:
        return assertNever(outcome)
    }
  }
}
