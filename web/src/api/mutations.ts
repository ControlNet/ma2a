import ky from "ky"

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
  ) {
    super(remediation ? `${code}: ${remediation}` : code)
  }
}

export type RuntimeMutationClient = {
  readonly createSpace: (name: string) => Promise<MutationSpaceView>
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

  return {
    createSpace: async (name) => {
      const result = await mutate("/api/v1/spaces/create", {
        version: LOCAL_API_VERSION,
        operation: "space_create",
        request_id: id(),
        name,
      })
      if (result.type !== "space_created") throw new RuntimeMutationPayloadError()
      return result.payload
    },
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
