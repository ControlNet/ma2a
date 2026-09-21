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
const SpaceMemberViewSchema = z.strictObject({
  endpoint_id: EndpointIdSchema,
  label: z.string().min(1).max(64),
  echo: z.boolean(),
  relay_provider: z.boolean(),
})
const SpaceViewSchema = z.strictObject({
  space_id: SpaceIdSchema,
  name: z.string().min(1).max(128),
  member_count: U32Schema,
  generation: RevisionSchema,
  chain_hash: z.string().regex(/^[0-9a-f]{64}$/),
  revoked_count: U32Schema,
})
const SpaceDetailsSchema = z
  .strictObject({
    revision: RevisionSchema,
    space: SpaceViewSchema,
    members: z.array(SpaceMemberViewSchema).max(64).readonly(),
  })
  .refine(
    (detail) =>
      detail.members.length === detail.space.member_count &&
      detail.members.every(
        (member, index) =>
          index === 0 || (detail.members[index - 1]?.endpoint_id ?? "") < member.endpoint_id,
      ),
    "members must be complete, sorted and unique",
  )
export type SpaceDetails = z.infer<typeof SpaceDetailsSchema>

export function parseSpaceDetails(value: unknown): SpaceDetails {
  const result = SpaceDetailsSchema.safeParse(value)
  if (!result.success) {
    throw new RuntimeApiPayloadError(result.error.issues.map((issue) => issue.message))
  }
  return result.data
}

const ControlSyncViewSchema = z.strictObject({
  peer_endpoint_ids: z.array(EndpointIdSchema).max(256).readonly(),
})
const ConnectionObservationViewSchema = z.strictObject({
  observed_at_ms: z.number().int().nonnegative(),
  path: z.enum(["connecting", "direct", "relay", "mixed_or_unknown"]),
  rtt_ms: z.number().int().nonnegative().nullable(),
  error_class: z.enum([
    "none",
    "transient",
    "authorization",
    "version",
    "revocation",
    "malformed_input",
    "policy",
    "cancelled",
  ]),
})
const ConnectionViewSchema = z.strictObject({
  endpoint_id: EndpointIdSchema,
  state: z.enum(["connecting", "connected", "failed"]),
  path: z.enum(["direct", "relay", "mixed_or_unknown"]),
  rtt_ms: z.number().int().nonnegative().nullable(),
  observations: z.array(ConnectionObservationViewSchema).max(8).readonly(),
})
const PrivateRelayCandidateViewSchema = z.strictObject({
  provider_endpoint_id: EndpointIdSchema,
  relay_url: z.string().min(1).max(2048),
  covered_space_ids: z.array(SpaceIdSchema).max(256).readonly(),
  home_compatible: z.boolean(),
})
const PublicRelayFallbackViewSchema = z.strictObject({
  relay_url: z.string().min(1).max(2048),
  enabled: z.boolean(),
  observed_connected: z.boolean(),
})
const ControlRoundViewSchema = z.strictObject({
  at_ms: z.number().int().nonnegative(),
  peer_count: U32Schema,
  outcome: z.enum(["succeeded", "failed", "empty"]),
})
const RuntimeSnapshotSchema = z
  .strictObject({
    revision: RevisionSchema,
    endpoint: EndpointViewSchema,
    spaces: z.array(SpaceViewSchema).max(256).readonly(),
    control_sync: ControlSyncViewSchema,
    connections: z.array(ConnectionViewSchema).max(256).readonly(),
    private_relay_candidates: z.array(PrivateRelayCandidateViewSchema).max(256).readonly(),
    public_relay_fallbacks: z.array(PublicRelayFallbackViewSchema).max(256).readonly(),
    control_rounds: z.array(ControlRoundViewSchema).max(16).readonly(),
    observed_relay_state: z.strictObject({
      private_relay_provider_running: z.boolean(),
      public_relay_connected: z.boolean(),
    }),
    reachability: z.strictObject({
      state: z.enum([
        "NoActiveSpaces",
        "DegradedNoCommonHome",
        "AwaitingIrohHome",
        "IrohHomeConnected",
      ]),
      direct: z.boolean(),
      relayed: z.boolean(),
    }),
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
