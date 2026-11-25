import { z } from "zod"

import type { RequestId } from "./generated"
import { RequestIdSchema } from "./mutations-schema"

export const PENDING_SPACE_STORAGE_KEY = "ma2a-pending-space-create"

const PendingSpaceCreationSchema = z.strictObject({
  endpointId: z.string().min(1).max(128),
  name: z.string().min(1).max(128),
  requestId: RequestIdSchema,
})

/** Non-secret idempotency state retained across a same-tab reload for one Runtime. */
export type PendingSpaceCreation = {
  readonly endpointId: string
  readonly name: string
  readonly requestId: RequestId
}

export function browserSessionStorage(): Storage | undefined {
  try {
    return typeof window === "undefined" ? undefined : window.sessionStorage
  } catch {
    return undefined
  }
}

export function readPendingSpaceCreation(
  endpointId: string | undefined,
  storage: Storage | undefined,
): PendingSpaceCreation | undefined {
  if (endpointId === undefined || storage === undefined) return undefined
  let raw: string | null
  try {
    raw = storage.getItem(PENDING_SPACE_STORAGE_KEY)
  } catch {
    return undefined
  }
  if (raw === null) return undefined
  let parsedJson: unknown
  try {
    parsedJson = JSON.parse(raw)
  } catch {
    clearPendingSpaceCreation(storage)
    return undefined
  }
  const parsed = PendingSpaceCreationSchema.safeParse(parsedJson)
  if (!parsed.success) {
    clearPendingSpaceCreation(storage)
    return undefined
  }
  if (parsed.data.endpointId !== endpointId) return undefined
  return {
    endpointId: parsed.data.endpointId,
    name: parsed.data.name,
    requestId: parsed.data.requestId as RequestId,
  }
}

export function writePendingSpaceCreation(
  entry: PendingSpaceCreation,
  storage: Storage | undefined,
): void {
  try {
    storage?.setItem(PENDING_SPACE_STORAGE_KEY, JSON.stringify(entry))
  } catch {
    return
  }
}

export function clearPendingSpaceCreation(storage: Storage | undefined): void {
  try {
    storage?.removeItem(PENDING_SPACE_STORAGE_KEY)
  } catch {
    return
  }
}

export type PendingSpaceTrackerOptions = {
  readonly endpointId?: (() => string | undefined) | undefined
  readonly storage?: Storage | undefined
  readonly onChange?: ((pending: PendingSpaceCreation | undefined) => void) | undefined
}

export type PendingSpaceTracker = {
  readonly begin: (
    name: string,
    nextId: () => RequestId,
  ) => { readonly requestId: RequestId; readonly reused: boolean }
  readonly markUnsettled: (requestId: RequestId) => void
  readonly settle: (requestId: RequestId) => void
  readonly discard: () => void
}

/**
 * Tracks one unresolved Space creation across retries and (when Runtime identity and tab storage
 * are supplied) across same-tab reloads.
 */
export function createPendingSpaceTracker(
  options: PendingSpaceTrackerOptions,
): PendingSpaceTracker {
  let memory:
    | {
        readonly endpointId: string | undefined
        readonly name: string
        readonly requestId: RequestId
      }
    | undefined
  const load = (scope: string | undefined) =>
    (scope !== undefined && options.storage !== undefined
      ? readPendingSpaceCreation(scope, options.storage)
      : undefined) ?? (memory?.endpointId === scope ? memory : undefined)
  const clear = (expected?: RequestId): void => {
    const current = load(options.endpointId?.())
    if (expected !== undefined && current?.requestId !== expected) return
    memory = undefined
    if (options.storage !== undefined) clearPendingSpaceCreation(options.storage)
    options.onChange?.(undefined)
  }
  return {
    begin: (name, nextId) => {
      const scope = options.endpointId?.()
      const current = load(scope)
      const reused = current !== undefined && current.name === name
      const requestId =
        current !== undefined && current.name === name ? current.requestId : nextId()
      memory = { endpointId: scope, name, requestId }
      if (scope !== undefined && options.storage !== undefined) {
        writePendingSpaceCreation({ endpointId: scope, name, requestId }, options.storage)
      }
      return { requestId, reused }
    },
    markUnsettled: (requestId) => {
      const scope = options.endpointId?.()
      const current = load(scope)
      if (scope !== undefined && current?.requestId === requestId) {
        options.onChange?.({ endpointId: scope, name: current.name, requestId })
      }
    },
    settle: (requestId) => clear(requestId),
    discard: () => clear(),
  }
}
