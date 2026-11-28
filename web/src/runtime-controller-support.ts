import type { RuntimeSnapshot, SpaceDetails, StampedRuntimeSnapshot } from "./api/client"

export type RuntimeRetryScheduler = {
  readonly schedule: (delayMs: number, task: () => void) => () => void
}

export const RETRY_DELAYS_MS = [250, 1_000, 4_000] as const

export const browserRetryScheduler: RuntimeRetryScheduler = {
  schedule: (delayMs, task) => {
    const timeout = window.setTimeout(task, delayMs)
    return () => window.clearTimeout(timeout)
  },
}

export function isSnapshotAtLeast(
  candidate: StampedRuntimeSnapshot,
  current: StampedRuntimeSnapshot | undefined,
): boolean {
  if (current === undefined || candidate.bootId !== current.bootId) return true
  if (candidate.snapshot.revision !== current.snapshot.revision) {
    return candidate.snapshot.revision > current.snapshot.revision
  }
  return candidate.snapshot.view_sequence >= current.snapshot.view_sequence
}

export function detailMatchesSpace(
  detail: SpaceDetails,
  space: RuntimeSnapshot["spaces"][number],
): boolean {
  return (
    detail.space.space_id === space.space_id &&
    detail.space.chain_hash === space.chain_hash &&
    detail.space.member_count === space.member_count &&
    detail.space.generation === space.generation
  )
}

export function retryDelayMs(attempt: number, hasSnapshot: boolean): number | undefined {
  return hasSnapshot
    ? RETRY_DELAYS_MS[Math.min(attempt, RETRY_DELAYS_MS.length - 1)]
    : RETRY_DELAYS_MS[attempt]
}
