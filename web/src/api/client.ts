import ky from "ky"
import { z } from "zod"

import {
  beginResnapshot,
  EMPTY_RUNTIME_STATE,
  installSnapshot,
  markDisconnected,
  type RuntimeState,
  receiveRevision,
} from "./state"

const RuntimeSnapshotSchema = z
  .strictObject({
    revision: z.number().int().nonnegative(),
    endpoint: z.unknown(),
    spaces: z.unknown(),
    control_sync: z.unknown(),
    relay_candidates: z.unknown(),
    observed_relay_state: z.unknown(),
    reachability: z.unknown(),
    recent_echo_summary: z.unknown(),
    ui_auth: z.unknown(),
  })
  .readonly()

const RuntimeEventSchema = z.looseObject({
  revision: z.number().int().nonnegative(),
})

const SameOriginPathSchema = z.string().regex(/^\/(?!\/)/)

export type RuntimeSnapshot = z.infer<typeof RuntimeSnapshotSchema>

export class RuntimeApiPayloadError extends Error {
  readonly name = "RuntimeApiPayloadError"

  constructor(readonly issues: readonly string[]) {
    super(`Runtime API payload is invalid: ${issues.join(", ")}`)
  }
}

export class RuntimeApiPathError extends Error {
  readonly name = "RuntimeApiPathError"

  constructor(readonly path: string) {
    super(`Runtime API path must be same-origin: ${path}`)
  }
}

export function parseRuntimeSnapshot(value: unknown): RuntimeSnapshot {
  const result = RuntimeSnapshotSchema.safeParse(value)
  if (!result.success) {
    throw new RuntimeApiPayloadError(result.error.issues.map((issue) => issue.message))
  }
  return result.data
}

function parseSameOriginPath(path: string): string {
  const result = SameOriginPathSchema.safeParse(path)
  if (!result.success) {
    throw new RuntimeApiPathError(path)
  }
  return result.data
}

function assertNever(value: never): never {
  throw new TypeError(`Unexpected recovery outcome: ${JSON.stringify(value)}`)
}

export type RuntimeApiPaths = {
  readonly snapshot: string
  readonly events: string
}

export type RuntimeEventCallbacks = {
  readonly onRevision: (revision: number) => void
  readonly onDisconnect: () => void
  readonly onPayloadError: (error: RuntimeApiPayloadError) => void
}

export type RuntimeApiClient = {
  readonly fetchSnapshot: () => Promise<RuntimeSnapshot>
  readonly subscribe: (callbacks: RuntimeEventCallbacks) => () => void
}

export function createRuntimeApiClient(paths: RuntimeApiPaths): RuntimeApiClient {
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
    subscribe: (callbacks) => {
      const source = new EventSource(eventsPath, { withCredentials: true })
      source.addEventListener("message", (message) => {
        const payload: unknown = JSON.parse(message.data)
        const result = RuntimeEventSchema.safeParse(payload)
        if (!result.success) {
          callbacks.onPayloadError(
            new RuntimeApiPayloadError(result.error.issues.map((issue) => issue.message)),
          )
          return
        }
        callbacks.onRevision(result.data.revision)
      })
      source.addEventListener("error", callbacks.onDisconnect)
      return () => source.close()
    },
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

  async disconnect(): Promise<void> {
    const transition = markDisconnected(this.state)
    this.state = transition.state
    if (transition.effect === "resnapshot") {
      await this.recover()
    }
  }

  private async recover(): Promise<void> {
    this.state = beginResnapshot(this.state)
    const outcome = await this.options.fetchSnapshot().then(
      (snapshot) => ({ kind: "success", snapshot }) as const,
      (error: unknown) => ({ kind: "failure", error }) as const,
    )
    switch (outcome.kind) {
      case "success":
        this.install(outcome.snapshot)
        return
      case "failure":
        this.options.onRecoveryError(outcome.error)
        return
      default:
        return assertNever(outcome)
    }
  }
}
