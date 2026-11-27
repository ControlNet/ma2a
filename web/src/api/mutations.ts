import ky, { HTTPError } from "ky"
import { z } from "zod"

import { type EndpointId, LOCAL_API_VERSION, type RequestId, type SpaceId } from "./generated"
import {
  EndpointIdSchema,
  type MutationControlSyncView,
  type MutationEchoReplyView,
  MutationErrorResponseSchema,
  type MutationPrivateRelayView,
  type MutationPublicRelayView,
  MutationResponseSchema,
  type MutationResult,
  type MutationSpaceView,
  type MutationUiAuthView,
  type PrivateRelayConfiguration,
  RequestIdSchema,
  SpaceIdSchema,
} from "./mutations-schema"
import {
  createPendingSpaceTracker,
  type PendingSpaceCreation,
  type PendingSpaceTracker,
} from "./pending-space"

export type {
  MutationControlSyncView,
  MutationEchoReplyView,
  MutationPrivateRelayView,
  MutationPublicRelayView,
  MutationSpaceView,
  MutationUiAuthView,
  PrivateRelayConfiguration,
} from "./mutations-schema"

export class RuntimeMutationPayloadError extends Error {
  readonly name = "RuntimeMutationPayloadError"
}

export class RuntimeMutationRejectedError extends Error {
  readonly name = "RuntimeMutationRejectedError"

  constructor(
    readonly code: string,
    readonly remediation?: string,
    readonly outcomeUnknown = code === "unavailable" && remediation === undefined,
  ) {
    super(remediation ? `${code}: ${remediation}` : code)
  }
}

/**
 * Whether a failed mutation may still have taken effect: no answer came back, or none that says
 * it was refused. Client-side input validation fails before any request leaves the browser; a
 * rejection, or an HTTP refusal before the command ran, is definite; a request timeout is not,
 * since the server can time out after it passed the command on, and a bare `unavailable` from
 * the Runtime is also what a pending replay answers.
 */
export function mutationOutcomeUnknown(error: unknown): boolean {
  if (error instanceof z.ZodError) return false
  if (error instanceof RuntimeMutationRejectedError) {
    return error.outcomeUnknown
  }
  return !(
    error instanceof HTTPError &&
    error.response.status < 500 &&
    error.response.status !== 408
  )
}

/**
 * What to tell the operator about a failed mutation: a client-side validation error when the
 * input failed before dispatch, `unknown` when it may still have taken effect, otherwise the
 * Runtime's remediation, or `rejected` when it gave none.
 */
export function mutationFailureText(error: unknown, unknown: string, rejected: string): string {
  if (error instanceof z.ZodError) return error.issues[0]?.message ?? rejected
  if (mutationOutcomeUnknown(error)) return unknown
  return error instanceof RuntimeMutationRejectedError && error.remediation
    ? error.remediation
    : rejected
}

export type RuntimeMutationClient = {
  readonly createSpace: (name: string) => Promise<MutationSpaceView>
  readonly discardPendingSpaceCreation: () => void
  readonly createInvitation: (
    spaceId: string,
    ttlMs: number,
    outputPath: string,
  ) => Promise<MutationSpaceView>
  readonly revokeEndpoint: (spaceId: string, endpointId: string) => Promise<MutationSpaceView>
  readonly triggerSync: (endpointId: string) => Promise<MutationControlSyncView>
  readonly configurePrivateRelay: (
    configuration: PrivateRelayConfiguration,
  ) => Promise<MutationPrivateRelayView>
  readonly configurePublicRelay: (url: string) => Promise<MutationPublicRelayView>
  readonly echo: (endpointId: string, payload: string) => Promise<MutationEchoReplyView>
  readonly revokeSessions: () => Promise<MutationUiAuthView>
}

type MutationClientOptions = {
  /** Read for every request, so a sign-in in another tab, which replaces it, is picked up. */
  readonly csrfToken: () => string
  readonly requestId?: () => string
  /** Runtime identity for scoping an unresolved Space creation in tab storage. */
  readonly endpointId?: () => string | undefined
  /** Tab storage used to retain an unresolved Space creation across page reloads. */
  readonly storage?: Storage | undefined
  /** Called when the unresolved Space creation settles, fails unsettled, or is discarded. */
  readonly onPendingSpaceCreation?: (pending: PendingSpaceCreation | undefined) => void
  /** Optional session-scoped tracker shared across controller replacements. */
  readonly pendingSpace?: PendingSpaceTracker | undefined
}

function browserRequestId(): string {
  return Array.from(crypto.getRandomValues(new Uint8Array(16)), (byte) =>
    byte.toString(16).padStart(2, "0"),
  ).join("")
}

export function createRuntimeMutationClient(options: MutationClientOptions): RuntimeMutationClient {
  const requestId = options.requestId ?? browserRequestId
  const http = ky.create({
    credentials: "same-origin",
    headers: { "content-type": "application/json" },
    retry: 0,
    timeout: 120_000,
  })
  const mutate = async (
    path: string,
    command: Record<string, unknown>,
  ): Promise<MutationResult> => {
    const payload: unknown = await http
      .post(new URL(path, window.location.origin), {
        json: command,
        headers: { "x-csrf-token": options.csrfToken() },
      })
      .json()
    const parsed = MutationResponseSchema.safeParse(payload)
    if (parsed.success) return parsed.data.result
    const rejected = MutationErrorResponseSchema.safeParse(payload)
    if (rejected.success) {
      throw new RuntimeMutationRejectedError(
        rejected.data.error,
        rejected.data.remediation ?? undefined,
      )
    }
    throw new RuntimeMutationPayloadError()
  }
  const id = (): RequestId => RequestIdSchema.parse(requestId()) as RequestId
  const endpoint = (value: string): EndpointId => EndpointIdSchema.parse(value) as EndpointId
  const space = (value: string): SpaceId => SpaceIdSchema.parse(value) as SpaceId
  // A creation without a definite answer may have committed. Retrying the same name reuses its
  // request ID, so the Runtime replays that outcome instead of creating a second Space. When tab
  // storage and Runtime identity are provided, the unresolved record also survives a page reload.
  // Only the Runtime's answer for that ID settles it: an HTTP refusal of a retry says nothing
  // about the earlier attempt, and `unavailable` means the Runtime is still running it.
  const pendingSpace =
    options.pendingSpace ??
    createPendingSpaceTracker({
      endpointId: options.endpointId,
      storage: options.storage,
      onChange: options.onPendingSpaceCreation,
    })

  return {
    createSpace: async (name) => {
      const { requestId, reused } = pendingSpace.begin(name, id)
      const result = await mutate("/api/v1/spaces/create", {
        version: LOCAL_API_VERSION,
        operation: "space_create",
        request_id: requestId,
        name,
      }).catch((error: unknown) => {
        const runtimeSettled =
          error instanceof RuntimeMutationRejectedError && error.code !== "unavailable"
        if (runtimeSettled || (!reused && !mutationOutcomeUnknown(error))) {
          pendingSpace.settle(requestId)
        } else {
          pendingSpace.markUnsettled(requestId)
        }
        throw error
      })
      if (result.type !== "space_created") {
        pendingSpace.markUnsettled(requestId)
        throw new RuntimeMutationPayloadError()
      }
      pendingSpace.settle(requestId)
      return result.payload
    },
    discardPendingSpaceCreation: () => pendingSpace.discard(),
    createInvitation: async (spaceId, ttlMs, outputPath) => {
      const result = await mutate("/api/v1/spaces/invite", {
        version: LOCAL_API_VERSION,
        operation: "space_invite",
        request_id: id(),
        space_id: space(spaceId),
        ttl_ms: ttlMs,
        output_path: outputPath,
      })
      if (result.type !== "space_invitation_created") throw new RuntimeMutationPayloadError()
      return result.payload
    },
    revokeEndpoint: async (spaceId, endpointId) => {
      const result = await mutate("/api/v1/spaces/revoke", {
        version: LOCAL_API_VERSION,
        operation: "space_revoke",
        request_id: id(),
        space_id: space(spaceId),
        peer_endpoint_id: endpoint(endpointId),
      })
      if (result.type !== "space_revoked") throw new RuntimeMutationPayloadError()
      return result.payload
    },
    triggerSync: async (endpointId) => {
      const result = await mutate("/api/v1/control-sync/trigger", {
        version: LOCAL_API_VERSION,
        operation: "control_sync_trigger",
        request_id: id(),
        peer_endpoint_id: endpoint(endpointId),
      }).catch((error: unknown) => {
        // Control sync uses a fresh request ID per trigger and never replays a pending entry, so
        // bare `unavailable` means no peer was dialed (no-effect refusal) rather than pending.
        if (
          error instanceof RuntimeMutationRejectedError &&
          error.code === "unavailable" &&
          error.remediation === undefined
        ) {
          throw new RuntimeMutationRejectedError(error.code, error.remediation, false)
        }
        throw error
      })
      if (result.type !== "control_sync_triggered") throw new RuntimeMutationPayloadError()
      return result.payload
    },
    configurePrivateRelay: async (configuration) => {
      const result = await mutate("/api/v1/relays/private/configure", {
        version: LOCAL_API_VERSION,
        operation: "private_relay_configure",
        request_id: id(),
        mode: configuration.mode,
        listen: configuration.listen,
        public_url: configuration.publicUrl,
        served_space_ids: configuration.servedSpaceIds.map(space),
        certificate_path: configuration.certificatePath,
        private_key_path: configuration.privateKeyPath,
      })
      if (result.type !== "private_relay_configured") throw new RuntimeMutationPayloadError()
      return result.payload
    },
    configurePublicRelay: async (url) => {
      const result = await mutate("/api/v1/relays/public/configure", {
        version: LOCAL_API_VERSION,
        operation: "public_relay_configure",
        request_id: id(),
        url,
      })
      if (result.type !== "public_relay_configured") throw new RuntimeMutationPayloadError()
      return result.payload
    },
    echo: async (endpointId, payload) => {
      const result = await mutate("/api/v1/echo", {
        version: LOCAL_API_VERSION,
        operation: "echo_call",
        request_id: id(),
        target_endpoint_id: endpoint(endpointId),
        payload,
      })
      if (result.type !== "echo") throw new RuntimeMutationPayloadError()
      return result.payload
    },
    revokeSessions: async () => {
      const result = await mutate("/api/v1/sessions/revoke-all", {
        version: LOCAL_API_VERSION,
        operation: "session_revoke_all",
        request_id: id(),
      })
      if (result.type !== "sessions_revoked") throw new RuntimeMutationPayloadError()
      return result.payload
    },
  }
}
