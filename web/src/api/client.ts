import ky from "ky"
import { z } from "zod"
import {
  parseRuntimeEvent,
  parseSpaceDetails,
  RuntimeApiPayloadError,
  type RuntimeEvent,
  type SpaceDetails,
} from "./codec"
import { readSnapshot, type StampedRuntimeSnapshot } from "./snapshot-stream"

export {
  parseRuntimeEvent,
  parseRuntimeSnapshot,
  parseSpaceDetails,
  RuntimeApiPayloadError,
  type RuntimeEvent,
  type RuntimeSnapshot,
  type SpaceDetails,
} from "./codec"
export {
  RuntimeStateCoordinator,
  type RuntimeStateCoordinatorOptions,
  StaleRuntimeSnapshotError,
} from "./coordinator"
export {
  CSRF_COOKIE_HEADER,
  RUNTIME_BOOT_ID_HEADER,
  type StampedRuntimeSnapshot,
} from "./snapshot-stream"

// A path is resolved as the browser resolves it, which reads a backslash as a slash and drops
// tabs and newlines. Checking two distinct origins ensures any path carrying its own host fails at
// least one, and malformed URLs reject cleanly as RuntimeApiPathError.
const PATH_BASES = ["http://a.invalid", "http://b.invalid"] as const

function isSameOriginPath(path: string): boolean {
  try {
    return PATH_BASES.every((base) => new URL(path, base).origin === base)
  } catch {
    return false
  }
}

const SameOriginPathSchema = z
  .string()
  .regex(/^\/(?!\/)/)
  .refine(isSameOriginPath)

export class RuntimeApiPathError extends Error {
  readonly name = "RuntimeApiPathError"

  constructor(readonly path: string) {
    super(`Runtime API path must be same-origin: ${path}`)
  }
}

function parseSameOriginPath(path: string): string {
  const result = SameOriginPathSchema.safeParse(path)
  if (!result.success) {
    throw new RuntimeApiPathError(path)
  }
  return result.data
}

export type RuntimeApiPaths = {
  readonly snapshot: string
  readonly events: string
}

export type RuntimeEventCallbacks = {
  /** The stream is open: the Runtime accepted it. */
  readonly onOpen: () => void
  readonly onEvent: (event: RuntimeEvent) => void
  readonly onResyncRequired: () => void
  readonly onDisconnect: () => void
  readonly onPayloadError: (error: RuntimeApiPayloadError) => void
}

export type RuntimeEventSource = {
  readonly addEventListener: (type: string, listener: (event: Event) => void) => void
  readonly close: () => void
}

export type RuntimeEventSourceFactory = (url: string, init: EventSourceInit) => RuntimeEventSource

export type RuntimeApiClient = {
  readonly fetchSpaceDetails: (spaceId: string) => Promise<SpaceDetails>
  readonly fetchSnapshot: () => Promise<StampedRuntimeSnapshot>
  readonly subscribe: (
    revision: number,
    bootId: string,
    callbacks: RuntimeEventCallbacks,
    viewSequence?: number,
  ) => () => void
}

const SNAPSHOT_TIMEOUT_MS = 30_000

const createBrowserEventSource: RuntimeEventSourceFactory = (url, init) =>
  new EventSource(url, init)

export function createRuntimeApiClient(
  paths: RuntimeApiPaths,
  createEventSource: RuntimeEventSourceFactory = createBrowserEventSource,
): RuntimeApiClient {
  const snapshotPath = parseSameOriginPath(paths.snapshot)
  const eventsPath = parseSameOriginPath(paths.events)
  const http = ky.create({
    credentials: "same-origin",
    headers: { accept: "application/json" },
    retry: 0,
    timeout: SNAPSHOT_TIMEOUT_MS,
  })

  return {
    fetchSpaceDetails: async (spaceId) => {
      if (!/^[0-9a-f]{64}$/.test(spaceId)) throw new RuntimeApiPathError(spaceId)
      return parseSpaceDetails(await http.get(`/api/v1/spaces/${spaceId}`).json())
    },
    fetchSnapshot: async () => {
      const controller = new AbortController()
      const timer = setTimeout(() => controller.abort(), SNAPSHOT_TIMEOUT_MS)
      try {
        const response = await http.get(new URL(snapshotPath, window.location.origin), {
          signal: controller.signal,
          timeout: false,
        })
        return await readSnapshot(response, controller.signal)
      } finally {
        clearTimeout(timer)
      }
    },
    subscribe: (revision, bootId, callbacks, viewSequence) => {
      const params = new URLSearchParams({ since: String(revision), boot_id: bootId })
      if (viewSequence !== undefined) {
        params.set("view_sequence", String(viewSequence))
      }
      const source = createEventSource(`${eventsPath}?${params.toString()}`, {
        withCredentials: true,
      })
      let closed = false
      const close = (): void => {
        if (closed) return
        closed = true
        source.close()
      }
      source.addEventListener("open", () => {
        if (closed) return
        callbacks.onOpen()
      })
      source.addEventListener("message", (message) => {
        if (closed) return
        if (!(message instanceof MessageEvent)) {
          close()
          callbacks.onPayloadError(new RuntimeApiPayloadError(["event data is not a message"]))
          callbacks.onResyncRequired()
          return
        }
        let event: RuntimeEvent
        try {
          const payload: unknown = JSON.parse(message.data)
          event = parseRuntimeEvent(payload)
        } catch (error) {
          if (!(error instanceof RuntimeApiPayloadError) && !(error instanceof SyntaxError)) {
            throw error
          }
          close()
          const payloadError =
            error instanceof RuntimeApiPayloadError
              ? error
              : new RuntimeApiPayloadError(["event data is not valid JSON"])
          callbacks.onPayloadError(payloadError)
          callbacks.onResyncRequired()
          return
        }
        callbacks.onEvent(event)
      })
      source.addEventListener("resync-required", () => {
        if (closed) return
        close()
        callbacks.onResyncRequired()
      })
      source.addEventListener("error", () => {
        if (closed) return
        close()
        callbacks.onDisconnect()
      })
      return close
    },
  }
}
