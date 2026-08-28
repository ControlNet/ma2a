export type UncertaintyReason =
  | "disconnected"
  | "duplicate_revision"
  | "event_before_snapshot"
  | "out_of_order_revision"
  | "revision_gap"

export type RuntimeState =
  | { readonly kind: "empty" }
  | { readonly kind: "ready"; readonly revision: number }
  | {
      readonly kind: "uncertain"
      readonly lastRevision: number | null
      readonly reason: UncertaintyReason
      readonly resnapshot: "required" | "in_flight"
    }

export type StateTransition = {
  readonly accepted: boolean
  readonly effect: "none" | "resnapshot"
  readonly state: RuntimeState
}

export const EMPTY_RUNTIME_STATE: RuntimeState = { kind: "empty" }

function assertNever(value: never): never {
  throw new Error(`Unexpected runtime state: ${JSON.stringify(value)}`)
}

function requireResnapshot(
  lastRevision: number | null,
  reason: UncertaintyReason,
): StateTransition {
  return {
    accepted: false,
    effect: "resnapshot",
    state: { kind: "uncertain", lastRevision, reason, resnapshot: "required" },
  }
}

export function installSnapshot(revision: number): RuntimeState {
  return { kind: "ready", revision }
}

export function beginResnapshot(state: RuntimeState): RuntimeState {
  switch (state.kind) {
    case "empty":
    case "ready":
      return state
    case "uncertain":
      if (state.resnapshot === "in_flight") {
        return state
      }
      return { ...state, resnapshot: "in_flight" }
    default:
      return assertNever(state)
  }
}

export function receiveRevision(state: RuntimeState, revision: number): StateTransition {
  switch (state.kind) {
    case "empty":
      return requireResnapshot(null, "event_before_snapshot")
    case "uncertain":
      return { accepted: false, effect: "none", state }
    case "ready": {
      const expectedRevision = state.revision + 1
      if (revision === expectedRevision) {
        return { accepted: true, effect: "none", state: { kind: "ready", revision } }
      }
      if (revision === state.revision) {
        return requireResnapshot(state.revision, "duplicate_revision")
      }
      if (revision < state.revision) {
        return requireResnapshot(state.revision, "out_of_order_revision")
      }
      return requireResnapshot(state.revision, "revision_gap")
    }
    default:
      return assertNever(state)
  }
}

export function markDisconnected(state: RuntimeState): StateTransition {
  switch (state.kind) {
    case "empty":
      return requireResnapshot(null, "disconnected")
    case "ready":
      return requireResnapshot(state.revision, "disconnected")
    case "uncertain":
      return { accepted: false, effect: "none", state }
    default:
      return assertNever(state)
  }
}
