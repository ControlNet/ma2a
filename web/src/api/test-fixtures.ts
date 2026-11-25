import type { RuntimeSnapshot, StampedRuntimeSnapshot } from "./client"

export const DEFAULT_BOOT_ID = "aa".repeat(16)

export function runtimeSnapshot(revision: number): RuntimeSnapshot {
  return {
    revision,
    endpoint: {
      endpoint_id: "00".repeat(32),
      runtime_version: "0.1.0",
      online: true,
    },
    spaces: [],
    control_sync: { peer_endpoint_ids: [] },
    connections: [],
    private_relay_candidates: [],
    public_relay_fallbacks: [],
    control_rounds: [],
    observed_relay_state: {
      private_relay_provider_running: false,
      public_relay_connected: false,
    },
    reachability: { state: "AwaitingIrohHome" as const, direct: true, relayed: false },
    recent_echo_summary: { successes: 0, failures: 0 },
    ui_auth: { initialized: true, password_set: false, active_sessions: 0 },
  }
}

export function stampedRuntimeSnapshot(
  revision: number,
  bootId = DEFAULT_BOOT_ID,
): StampedRuntimeSnapshot {
  return { snapshot: runtimeSnapshot(revision), bootId }
}

export function deferredRuntimeSnapshot(): {
  readonly promise: Promise<StampedRuntimeSnapshot>
  readonly resolve: (value: StampedRuntimeSnapshot) => void
} {
  let resolveSnapshot: ((value: StampedRuntimeSnapshot) => void) | undefined
  const promise = new Promise<StampedRuntimeSnapshot>((resolve) => {
    resolveSnapshot = resolve
  })
  if (resolveSnapshot === undefined) {
    throw new TypeError("Deferred snapshot resolver was not initialized")
  }
  return { promise, resolve: resolveSnapshot }
}
