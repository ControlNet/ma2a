import { z } from "zod"

import { LOCAL_API_VERSION } from "./generated"

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

export const RequestIdSchema = z
  .string()
  .regex(/^[0-9a-f]{32}$/, "Request ID must be 32 lowercase hexadecimal characters.")
export const EndpointIdSchema = z
  .string()
  .regex(/^[0-9a-f]{64}$/, "Endpoint ID must be 64 lowercase hexadecimal characters.")
export const SpaceIdSchema = z
  .string()
  .regex(/^[0-9a-f]{64}$/, "Space ID must be 64 lowercase hexadecimal characters.")
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

export type MutationResult =
  | { readonly type: "space_created"; readonly payload: MutationSpaceView }
  | { readonly type: "space_invitation_created"; readonly payload: MutationSpaceView }
  | { readonly type: "space_revoked"; readonly payload: MutationSpaceView }
  | { readonly type: "control_sync_triggered"; readonly payload: MutationControlSyncView }
  | { readonly type: "private_relay_configured"; readonly payload: MutationPrivateRelayView }
  | { readonly type: "public_relay_configured"; readonly payload: MutationPublicRelayView }
  | { readonly type: "echo"; readonly payload: MutationEchoReplyView }
  | { readonly type: "sessions_revoked"; readonly payload: MutationUiAuthView }

export const MutationResponseSchema = z.strictObject({
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

export const MutationErrorResponseSchema = z.strictObject({
  version: z.literal(LOCAL_API_VERSION),
  error: z.string().min(1),
  remediation: z.string().nullable().optional(),
})
