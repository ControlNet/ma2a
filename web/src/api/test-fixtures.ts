import type { RuntimeSnapshot } from "./client"

export function runtimeSnapshot(revision: number): RuntimeSnapshot {
  return {
    revision,
    endpoint: {
      endpoint_id: "00".repeat(32),
      runtime_version: "0.1.0",
      online: true,
    },
    spaces: [],
    control_sync: { peer_endpoint_ids: [], synchronized: true },
    relay_candidates: [],
    observed_relay_state: { private_relay_online: false, public_relay_online: false },
    reachability: { direct: true, relayed: false },
    recent_echo_summary: { successes: 0, failures: 0 },
    ui_auth: { initialized: true, password_set: false, active_sessions: 0 },
  }
}

export function deferredRuntimeSnapshot(): {
  readonly promise: Promise<RuntimeSnapshot>
  readonly resolve: (value: RuntimeSnapshot) => void
} {
  let resolveSnapshot: ((value: RuntimeSnapshot) => void) | undefined
  const promise = new Promise<RuntimeSnapshot>((resolve) => {
    resolveSnapshot = resolve
  })
  if (resolveSnapshot === undefined) {
    throw new TypeError("Deferred snapshot resolver was not initialized")
  }
  return { promise, resolve: resolveSnapshot }
}
