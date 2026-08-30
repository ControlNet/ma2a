import type { RuntimeEvent, RuntimeSnapshot } from "./codec"
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
  readonly fetchSnapshot: () => Promise<RuntimeSnapshot>
  readonly onIncremental: (revision: number) => void
  readonly onSnapshot: (snapshot: RuntimeSnapshot) => void
  readonly onRecoveryError: (error: unknown) => void
}

export class RuntimeStateCoordinator {
  private state: RuntimeState = EMPTY_RUNTIME_STATE

  constructor(private readonly options: RuntimeStateCoordinatorOptions) {}

  currentState(): RuntimeState {
    return this.state
  }

  install(snapshot: RuntimeSnapshot): void {
    this.state = installSnapshot(snapshot.revision)
    this.options.onSnapshot(snapshot)
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
    this.state = beginResnapshot(this.state)
    const outcome = await this.options.fetchSnapshot().then(
      (snapshot) => ({ kind: "success", snapshot }) as const,
      (error: unknown) => ({ kind: "failure", error }) as const,
    )
    switch (outcome.kind) {
      case "success":
        if (lastRevision !== null && outcome.snapshot.revision < lastRevision) {
          this.state = failResnapshot(this.state)
          this.options.onRecoveryError(
            new StaleRuntimeSnapshotError(lastRevision, outcome.snapshot.revision),
          )
          return
        }
        this.install(outcome.snapshot)
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
