import { z } from "zod"

const RevisionSchema = z.number().int().nonnegative()
const U32Schema = z.number().int().min(0).max(4_294_967_295)
const EndpointIdSchema = z.string().regex(/^[0-9a-f]{64}$/)
const SpaceIdSchema = z.string().regex(/^[0-9a-f]{64}$/)
const EndpointViewSchema = z.strictObject({
  endpoint_id: EndpointIdSchema,
  runtime_version: z.string().min(1).max(128),
  online: z.boolean(),
})
const SpaceViewSchema = z.strictObject({
  space_id: SpaceIdSchema,
  name: z.string().min(1).max(128),
  member_count: U32Schema,
})
const ControlSyncViewSchema = z.strictObject({
  peer_endpoint_ids: z.array(EndpointIdSchema).max(256).readonly(),
  synchronized: z.boolean(),
})
const RelayCandidateViewSchema = z.strictObject({
  endpoint_id: EndpointIdSchema,
  relay_kind: z.string().min(1).max(32),
  eligible: z.boolean(),
})
const RuntimeSnapshotSchema = z
  .strictObject({
    revision: RevisionSchema,
    endpoint: EndpointViewSchema,
    spaces: z.array(SpaceViewSchema).max(256).readonly(),
    control_sync: ControlSyncViewSchema,
    relay_candidates: z.array(RelayCandidateViewSchema).max(256).readonly(),
    observed_relay_state: z.strictObject({
      private_relay_online: z.boolean(),
      public_relay_online: z.boolean(),
    }),
    reachability: z.strictObject({ direct: z.boolean(), relayed: z.boolean() }),
    recent_echo_summary: z.strictObject({ successes: U32Schema, failures: U32Schema }),
    ui_auth: z.strictObject({
      initialized: z.boolean(),
      password_set: z.boolean(),
      active_sessions: U32Schema,
    }),
  })
  .readonly()

const EndpointChangesSchema = z.strictObject({
  endpoint_ids: z.array(EndpointIdSchema).max(256).readonly(),
})
const SpaceChangesSchema = z.strictObject({
  space_ids: z.array(SpaceIdSchema).max(256).readonly(),
})
const EmptyChangesSchema = z.strictObject({})
const RuntimeEventSchema = z.discriminatedUnion("type", [
  z.strictObject({
    type: z.literal("snapshot_invalidated"),
    revision: RevisionSchema,
    changed: EmptyChangesSchema,
  }),
  z.strictObject({
    type: z.literal("endpoint_changed"),
    revision: RevisionSchema,
    changed: EndpointChangesSchema,
  }),
  z.strictObject({
    type: z.literal("spaces_changed"),
    revision: RevisionSchema,
    changed: SpaceChangesSchema,
  }),
  z.strictObject({
    type: z.literal("control_sync_changed"),
    revision: RevisionSchema,
    changed: EndpointChangesSchema,
  }),
  z.strictObject({
    type: z.literal("relay_candidates_changed"),
    revision: RevisionSchema,
    changed: EndpointChangesSchema,
  }),
  z.strictObject({
    type: z.literal("relay_state_changed"),
    revision: RevisionSchema,
    changed: EmptyChangesSchema,
  }),
  z.strictObject({
    type: z.literal("reachability_changed"),
    revision: RevisionSchema,
    changed: EndpointChangesSchema,
  }),
  z.strictObject({
    type: z.literal("echo_summary_changed"),
    revision: RevisionSchema,
    changed: EndpointChangesSchema,
  }),
  z.strictObject({
    type: z.literal("ui_auth_changed"),
    revision: RevisionSchema,
    changed: EmptyChangesSchema,
  }),
])

export type RuntimeSnapshot = z.infer<typeof RuntimeSnapshotSchema>
export type RuntimeEvent = z.infer<typeof RuntimeEventSchema>

export class RuntimeApiPayloadError extends Error {
  readonly name = "RuntimeApiPayloadError"

  constructor(readonly issues: readonly string[]) {
    super(`Runtime API payload is invalid: ${issues.join(", ")}`)
  }
}

export function parseRuntimeSnapshot(value: unknown): RuntimeSnapshot {
  const result = RuntimeSnapshotSchema.safeParse(value)
  if (!result.success) {
    throw new RuntimeApiPayloadError(result.error.issues.map((issue) => issue.message))
  }
  return result.data
}

export function parseRuntimeEvent(value: unknown): RuntimeEvent {
  const result = RuntimeEventSchema.safeParse(value)
  if (!result.success) {
    throw new RuntimeApiPayloadError(result.error.issues.map((issue) => issue.message))
  }
  return result.data
}
