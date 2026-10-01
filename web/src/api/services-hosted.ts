import { z } from "zod"

const U64 = z.number().int().nonnegative()
const Version = z.number().int().min(1).max(65_535)

const OperationSchema = z.strictObject({
  name: z
    .string()
    .min(1)
    .max(32)
    .regex(/^[a-z][a-z0-9_.-]*$/),
  interaction: z.enum(["unary", "stream", "bytes", "resource"]),
})
const VersionSchema = z.strictObject({
  version: Version,
  operations: z.array(OperationSchema).min(1).max(16).readonly(),
})
/** A hosted service as the browser sees it: what it offers, its state, and its call counts. */
const HostedServiceSchema = z.strictObject({
  service: z
    .string()
    .min(1)
    .max(63)
    .regex(/^[a-z][a-z0-9-]*$/),
  description: z.string().max(256),
  versions: z.array(VersionSchema).min(1).max(8).readonly(),
  max_calls: z.number().int().min(1).max(64),
  enabled: z.boolean(),
  state: z.enum(["stopped", "unavailable", "ready"]),
  running_calls: U64,
  calls: z
    .array(z.strictObject({ version: Version, count: U64 }))
    .max(8)
    .readonly(),
})
export const HostedServicesSchema = z.strictObject({
  services: z.array(HostedServiceSchema).max(16).readonly(),
  more: z.boolean(),
})

export type HostedService = z.infer<typeof HostedServiceSchema>
export type HostedServicesView = z.infer<typeof HostedServicesSchema>
