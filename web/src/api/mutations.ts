import ky from "ky"
import { z } from "zod"

import { type EndpointId, LOCAL_API_VERSION, type RequestId, type SpaceId } from "./generated"
export type MutationSpaceView = {
  readonly space_id: string
  readonly name: string
  readonly member_count: number
}
export type MutationControlSyncView = {
  readonly peer_endpoint_ids: readonly string[]
}
export type MutationPrivateRelayView = {
  readonly configured: boolean
  readonly mode: "native_tls" | "external_termination"
  readonly host: string
  readonly port: number
  readonly online: boolean
}
export type MutationPublicRelayView = {
  readonly configured: boolean
  readonly url: string | null
  readonly online: boolean
}
export type MutationEchoReplyView = {
  readonly target_endpoint_id: string
  readonly payload: string
  readonly duration_ms: number
}
export type MutationUiAuthView = {
  readonly initialized: boolean
  readonly password_set: boolean
  readonly active_sessions: number
}
export type PrivateRelayConfiguration =
  | {
      readonly mode: "native_tls"
      readonly listen: string
      readonly publicUrl: string
      readonly servedSpaceIds: readonly string[]
      readonly certificatePath: string
      readonly privateKeyPath: string
    }
  | {
      readonly mode: "external_termination"
      readonly listen: string
      readonly publicUrl: string
      readonly servedSpaceIds: readonly string[]
      readonly certificatePath: null
      readonly privateKeyPath: null
    }

const RequestIdSchema = z.string().regex(/^[0-9a-f]{32}$/)
const EndpointIdSchema = z.string().regex(/^[0-9a-f]{64}$/)
const SpaceIdSchema = z.string().regex(/^[0-9a-f]{64}$/)
const SpaceSchema = z.strictObject({
  space_id: SpaceIdSchema,
  name: z.string().min(1).max(128),
  member_count: z.number().int().nonnegative(),
})
const ControlSyncSchema = z.strictObject({
  peer_endpoint_ids: z.array(EndpointIdSchema).max(256).readonly(),
})
const PrivateRelaySchema = z.strictObject({
  configured: z.boolean(),
  mode: z.union([z.literal("native_tls"), z.literal("external_termination")]),
  host: z.string(),
  port: z.number().int().min(0).max(65_535),
  online: z.boolean(),
})
const PublicRelaySchema = z.strictObject({
  configured: z.boolean(),
  url: z.string().nullable(),
  online: z.boolean(),
})
const EchoSchema = z.strictObject({
  target_endpoint_id: EndpointIdSchema,
  payload: z.string(),
  duration_ms: z.number().int().min(0).max(10_000),
})
const UiAuthSchema = z.strictObject({
  initialized: z.boolean(),
  password_set: z.boolean(),
  active_sessions: z.number().int().nonnegative(),
})

type MutationResult =
  | { readonly type: "space_created"; readonly payload: MutationSpaceView }
  | { readonly type: "space_invitation_created"; readonly payload: MutationSpaceView }
  | { readonly type: "space_revoked"; readonly payload: MutationSpaceView }
  | { readonly type: "control_sync_triggered"; readonly payload: MutationControlSyncView }
  | { readonly type: "private_relay_configured"; readonly payload: MutationPrivateRelayView }
  | { readonly type: "public_relay_configured"; readonly payload: MutationPublicRelayView }
  | { readonly type: "echo"; readonly payload: MutationEchoReplyView }
  | { readonly type: "sessions_revoked"; readonly payload: MutationUiAuthView }

const MutationResponseSchema = z.strictObject({
  version: z.literal(LOCAL_API_VERSION),
  request_id: RequestIdSchema.nullable(),
  revision: z.number().int().nonnegative(),
  result: z.discriminatedUnion("type", [
    z.strictObject({ type: z.literal("space_created"), payload: SpaceSchema }),
    z.strictObject({ type: z.literal("space_invitation_created"), payload: SpaceSchema }),
    z.strictObject({ type: z.literal("space_revoked"), payload: SpaceSchema }),
    z.strictObject({ type: z.literal("control_sync_triggered"), payload: ControlSyncSchema }),
    z.strictObject({ type: z.literal("private_relay_configured"), payload: PrivateRelaySchema }),
    z.strictObject({ type: z.literal("public_relay_configured"), payload: PublicRelaySchema }),
    z.strictObject({ type: z.literal("echo"), payload: EchoSchema }),
    z.strictObject({ type: z.literal("sessions_revoked"), payload: UiAuthSchema }),
  ]),
})

export class RuntimeMutationPayloadError extends Error {
  readonly name = "RuntimeMutationPayloadError"
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
  readonly csrfToken: string
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
    headers: { "content-type": "application/json", "x-csrf-token": options.csrfToken },
    retry: 0,
    timeout: 5_000,
  })
  const mutate = async (
    path: string,
    command: Record<string, unknown>,
  ): Promise<MutationResult> => {
    const payload: unknown = await http
      .post(new URL(path, window.location.origin), { json: command })
      .json()
    const parsed = MutationResponseSchema.safeParse(payload)
    if (!parsed.success) {
      throw new RuntimeMutationPayloadError()
    }
    return parsed.data.result
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
