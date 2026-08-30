import ky from "ky"
import { z } from "zod"

import {
  parseRuntimeEvent,
  parseRuntimeSnapshot,
  RuntimeApiPayloadError,
  type RuntimeEvent,
  type RuntimeSnapshot,
} from "./codec"

export {
  parseRuntimeEvent,
  parseRuntimeSnapshot,
  RuntimeApiPayloadError,
  type RuntimeEvent,
  type RuntimeSnapshot,
} from "./codec"
export {
  RuntimeStateCoordinator,
  type RuntimeStateCoordinatorOptions,
  StaleRuntimeSnapshotError,
} from "./coordinator"

const SameOriginPathSchema = z.string().regex(/^\/(?!\/)/)

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
  readonly fetchSnapshot: () => Promise<RuntimeSnapshot>
  readonly subscribe: (revision: number, callbacks: RuntimeEventCallbacks) => () => void
}

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
    timeout: 5_000,
  })

  return {
    fetchSnapshot: async () => {
      const payload: unknown = await http.get(snapshotPath).json()
      return parseRuntimeSnapshot(payload)
    },
    subscribe: (revision, callbacks) => {
      const source = createEventSource(`${eventsPath}?since=${revision}`, { withCredentials: true })
      source.addEventListener("message", (message) => {
        if (!(message instanceof MessageEvent)) {
          source.close()
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
          source.close()
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
        source.close()
        callbacks.onResyncRequired()
      })
      source.addEventListener("error", () => {
        source.close()
        callbacks.onDisconnect()
      })
      return () => source.close()
    },
  }
}
