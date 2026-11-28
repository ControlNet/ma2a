import { z } from "zod"

import { parseRuntimeSnapshot, RuntimeApiPayloadError, type RuntimeSnapshot } from "./codec"

export const RUNTIME_BOOT_ID_HEADER = "x-ma2a-runtime-boot-id"

const BootIdSchema = z.string().regex(/^[0-9a-f]{32}$/)

export type StampedRuntimeSnapshot = {
  readonly snapshot: RuntimeSnapshot
  readonly bootId: string
}

const FragmentSchema = z.strictObject({
  type: z.literal("snapshot_fragment"),
  revision: z.number().int().nonnegative(),
  runtime_boot_id: BootIdSchema,
  index: z.number().int().nonnegative(),
  last: z.boolean(),
  data_hex: z
    .string()
    .min(2)
    .max(32_768)
    .regex(/^(?:[0-9a-f]{2})+$/),
})

function invalid(reason: string): never {
  throw new RuntimeApiPayloadError([reason])
}

function parseBootHeader(response: Response): string | undefined {
  const raw = response.headers.get(RUNTIME_BOOT_ID_HEADER)
  if (raw === null) return undefined
  const parsed = BootIdSchema.safeParse(raw)
  if (!parsed.success) invalid("invalid runtime boot id")
  return parsed.data
}

// Publish only a complete, ordered frozen snapshot. A new request starts afresh;
// fragments from different revisions/boots are never combined.
export async function readSnapshot(response: Response): Promise<StampedRuntimeSnapshot> {
  const headerBoot = parseBootHeader(response)
  if (!response.headers.get("content-type")?.startsWith("application/x-ndjson")) {
    if (headerBoot === undefined) invalid("missing runtime boot id")
    return { snapshot: parseRuntimeSnapshot(await response.json()), bootId: headerBoot }
  }
  const reader = response.body?.getReader()
  if (reader === undefined) return invalid("missing snapshot stream")
  const decoder = new TextDecoder("utf-8", { fatal: true })
  const payloadDecoder = new TextDecoder("utf-8", { fatal: true })
  let buffered = ""
  let index = 0
  let revision: number | undefined
  let boot: string | undefined
  let complete = false
  const parts: string[] = []
  try {
    for (;;) {
      const { value, done } = await reader.read()
      if (done) break
      buffered += decoder.decode(value, { stream: true })
      for (let end = buffered.indexOf("\n"); end >= 0; end = buffered.indexOf("\n")) {
        const line = buffered.slice(0, end)
        buffered = buffered.slice(end + 1)
        if (line.length > 65_536) invalid("oversized snapshot fragment")
        const fragment = FragmentSchema.parse(JSON.parse(line))
        if (
          complete ||
          fragment.index !== index ||
          (revision !== undefined && revision !== fragment.revision) ||
          (boot !== undefined && boot !== fragment.runtime_boot_id) ||
          (!fragment.last && fragment.data_hex.length !== 32_768)
        ) {
          invalid("inconsistent snapshot stream")
        }
        revision = fragment.revision
        boot = fragment.runtime_boot_id
        index += 1
        const bytes = new Uint8Array(fragment.data_hex.length / 2)
        for (let offset = 0; offset < bytes.length; offset += 1) {
          bytes[offset] = Number.parseInt(fragment.data_hex.slice(offset * 2, offset * 2 + 2), 16)
        }
        // Hex fragments may split a UTF-8 character; use a distinct incremental decoder.
        parts.push(payloadDecoder.decode(bytes, { stream: !fragment.last }))
        complete = fragment.last
      }
      if (buffered.length > 65_536) invalid("oversized snapshot fragment")
    }
    buffered += decoder.decode()
    if (!complete || buffered.length !== 0 || boot === undefined) {
      invalid("incomplete snapshot stream")
    }
    if (headerBoot !== undefined && headerBoot !== boot) invalid("inconsistent snapshot stream")
    const snapshot = parseRuntimeSnapshot(JSON.parse(parts.join("")))
    if (snapshot.revision !== revision) invalid("snapshot revision mismatch")
    return { snapshot, bootId: boot }
  } catch (error) {
    if (error instanceof RuntimeApiPayloadError) throw error
    return invalid("invalid snapshot stream")
  } finally {
    try {
      await reader.cancel()
    } catch {
      // Stream errors are already normalized in catch; still release the reader lock.
    } finally {
      reader.releaseLock()
    }
  }
}
