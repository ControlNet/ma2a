import { z } from "zod"

import type { RequestId } from "./generated"
import { RequestIdSchema } from "./mutations-schema"

export const PENDING_SPACE_STORAGE_KEY = "ma2a-pending-space-create"

const PendingSpaceCreationSchema = z.strictObject({
  endpointId: z.string().min(1).max(128),
  name: z.string().min(1).max(128),
  requestId: RequestIdSchema,
})

const StoredPendingSpaceCreationsSchema = z.union([
  PendingSpaceCreationSchema,
  z.array(PendingSpaceCreationSchema).min(1),
])

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

function readPendingSpaceCreations(
  endpointId: string | undefined,
  storage: Storage | undefined,
): PendingSpaceCreation[] {
  if (endpointId === undefined || storage === undefined) return []
  let raw: string | null
  try {
    raw = storage.getItem(PENDING_SPACE_STORAGE_KEY)
  } catch {
    return []
  }
  if (raw === null) return []
  let parsedJson: unknown
  try {
    parsedJson = JSON.parse(raw)
  } catch {
    clearPendingSpaceCreation(storage)
    return []
  }
  const parsed = StoredPendingSpaceCreationsSchema.safeParse(parsedJson)
  if (!parsed.success) {
    clearPendingSpaceCreation(storage)
    return []
  }
  const entries = Array.isArray(parsed.data) ? parsed.data : [parsed.data]
  return entries
    .filter((entry) => entry.endpointId === endpointId)
    .map((entry) => ({
      endpointId: entry.endpointId,
      name: entry.name,
      requestId: entry.requestId as RequestId,
    }))
}

export function readPendingSpaceCreation(
  endpointId: string | undefined,
  storage: Storage | undefined,
): PendingSpaceCreation | undefined {
  return readPendingSpaceCreations(endpointId, storage).at(-1)
}

function writePendingSpaceCreations(
  entries: readonly PendingSpaceCreation[],
  storage: Storage | undefined,
): boolean {
  if (storage === undefined) return false
  if (entries.length === 0) {
    clearPendingSpaceCreation(storage)
    return true
  }
  try {
    const value = entries.length === 1 ? entries[0] : entries
    storage.setItem(PENDING_SPACE_STORAGE_KEY, JSON.stringify(value))
    return true
  } catch {
    return false
  }
}

export function writePendingSpaceCreation(
  entry: PendingSpaceCreation,
  storage: Storage | undefined,
): boolean {
  return writePendingSpaceCreations([entry], storage)
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
  readonly isActive?: (() => boolean) | undefined
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

type MemoryPendingSpace = {
  readonly endpointId: string | undefined
  readonly name: string
  readonly requestId: RequestId
}

/**
 * Tracks unresolved Space creations per name across retries and (when Runtime identity and tab
 * storage are supplied) across same-tab reloads.
 */
export function createPendingSpaceTracker(
  options: PendingSpaceTrackerOptions,
): PendingSpaceTracker {
  let memory: MemoryPendingSpace[] = []
  const loadedScopes = new Set<string | undefined>()
  const load = (scope: string | undefined): readonly MemoryPendingSpace[] => {
    if (loadedScopes.has(scope)) {
      return memory.filter((entry) => entry.endpointId === scope)
    }
    const stored =
      scope !== undefined && options.storage !== undefined
        ? readPendingSpaceCreations(scope, options.storage)
        : []
    if (stored.length > 0) {
      memory = [...memory.filter((entry) => entry.endpointId !== scope), ...stored]
      loadedScopes.add(scope)
      return stored
    }
    return memory.filter((entry) => entry.endpointId === scope)
  }
  const persist = (scope: string | undefined, entries: readonly MemoryPendingSpace[]): void => {
    loadedScopes.add(scope)
    memory = [...memory.filter((entry) => entry.endpointId !== scope), ...entries]
    if (scope !== undefined && options.storage !== undefined) {
      const toStore = entries.map((entry) => ({
        endpointId: scope,
        name: entry.name,
        requestId: entry.requestId,
      }))
      if (!writePendingSpaceCreations(toStore, options.storage)) {
        const currentIds = new Set(toStore.map((entry) => entry.requestId))
        const staleOnDisk = readPendingSpaceCreations(scope, options.storage).some(
          (entry) => !currentIds.has(entry.requestId),
        )
        if (staleOnDisk) clearPendingSpaceCreation(options.storage)
      }
    }
  }
  const active = (): boolean => options.isActive?.() ?? true
  const notify = (scope: string | undefined, entry: MemoryPendingSpace | undefined): void => {
    options.onChange?.(
      scope !== undefined && entry !== undefined
        ? { endpointId: scope, name: entry.name, requestId: entry.requestId }
        : undefined,
    )
  }
  const remove = (requestId: RequestId): void => {
    if (!active()) return
    const scope = options.endpointId?.()
    const current = load(scope)
    if (!current.some((entry) => entry.requestId === requestId)) return
    const remaining = current.filter((entry) => entry.requestId !== requestId)
    persist(scope, remaining)
    notify(scope, remaining.at(-1))
  }
  return {
    begin: (name, nextId) => {
      if (!active()) return { requestId: nextId(), reused: false }
      const scope = options.endpointId?.()
      const current = load(scope)
      const existing = current.find((entry) => entry.name === name)
      if (existing !== undefined) {
        return { requestId: existing.requestId, reused: true }
      }
      const requestId = nextId()
      persist(scope, [...current, { endpointId: scope, name, requestId }])
      return { requestId, reused: false }
    },
    markUnsettled: (requestId) => {
      if (!active()) return
      const scope = options.endpointId?.()
      const matched = load(scope).find((entry) => entry.requestId === requestId)
      if (scope !== undefined && matched !== undefined) {
        notify(scope, matched)
      }
    },
    settle: remove,
    discard: () => {
      if (!active()) return
      const scope = options.endpointId?.()
      persist(scope, [])
      notify(scope, undefined)
    },
  }
}
