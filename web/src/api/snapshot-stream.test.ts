import { expect, test, vi } from "vitest"

import { createRuntimeApiClient } from "./client"
import { RuntimeApiPayloadError } from "./codec"
import { RUNTIME_BOOT_ID_HEADER, readSnapshot } from "./snapshot-stream"
import { runtimeSnapshot as snapshot } from "./test-fixtures"

function frames(payload: unknown, revision = 7, bootId = "ab".repeat(16)): string[] {
  const bytes = new TextEncoder().encode(JSON.stringify(payload))
  const result: string[] = []
  for (let offset = 0; offset < bytes.length; offset += 16_384) {
    const chunk = bytes.subarray(offset, offset + 16_384)
    result.push(
      JSON.stringify({
        type: "snapshot_fragment",
        revision,
        runtime_boot_id: bootId,
        index: offset / 16_384,
        last: offset + chunk.length === bytes.length,
        data_hex: Array.from(chunk, (byte) => byte.toString(16).padStart(2, "0")).join(""),
      }),
    )
  }
  return result
}

function response(lines: readonly string[], bootHeader?: string): Response {
  const encoder = new TextEncoder()
  const wire = encoder.encode(`${lines.join("\n")}\n`)
  const headers: Record<string, string> = { "content-type": "application/x-ndjson" }
  if (bootHeader !== undefined) headers[RUNTIME_BOOT_ID_HEADER] = bootHeader
  return new Response(
    new ReadableStream({
      start(controller) {
        // Transport chunks intentionally differ from semantic fragment boundaries.
        for (let offset = 0; offset < wire.length; offset += 137) {
          controller.enqueue(wire.subarray(offset, offset + 137))
        }
        controller.close()
      },
    }),
    { headers },
  )
}

function largeSnapshot(revision = 7) {
  const base = snapshot(revision)
  return {
    ...base,
    spaces: Array.from({ length: 300 }, (_, index) => ({
      space_id: index.toString(16).padStart(64, "0"),
      name: '🦀"\\'.repeat(16),
      member_count: 64,
      generation: 10,
      chain_hash: "ff".repeat(32),
      revoked_count: 0,
    })),
  }
}

test("large frozen snapshots survive frame and UTF-8 boundaries without truncation", async () => {
  const expected = largeSnapshot()
  const bootId = "ab".repeat(16)
  const encoded = frames(expected, 7, bootId)
  expect(encoded.length).toBeGreaterThan(1)
  expect(encoded.every((frame) => frame.length < 33_000)).toBe(true)
  expect(await readSnapshot(response(encoded, bootId))).toEqual({ snapshot: expected, bootId })
})

test("small JSON snapshots require and return the validated Runtime boot ID header", async () => {
  const expected = snapshot(5)
  const bootId = "cd".repeat(16)
  const valid = Response.json(expected, { headers: { [RUNTIME_BOOT_ID_HEADER]: bootId } })

  expect(await readSnapshot(valid)).toEqual({ snapshot: expected, bootId })
  await expect(readSnapshot(Response.json(expected))).rejects.toThrow()
  await expect(
    readSnapshot(Response.json(expected, { headers: { [RUNTIME_BOOT_ID_HEADER]: "not-hex" } })),
  ).rejects.toThrow()
})

test("incomplete, reordered, mixed-revision, mixed-boot and extra frames never publish a snapshot", async () => {
  const encoded = frames(largeSnapshot())
  const first = encoded[0]
  const second = encoded[1]
  if (first === undefined || second === undefined) throw new Error("missing fixture frames")
  await expect(readSnapshot(response(encoded.slice(0, -1)))).rejects.toThrow()
  await expect(readSnapshot(response([second, first]))).rejects.toThrow()
  await expect(
    readSnapshot(response([first, JSON.stringify({ ...JSON.parse(second), revision: 8 })])),
  ).rejects.toThrow()
  await expect(
    readSnapshot(
      response([
        first,
        JSON.stringify({ ...JSON.parse(second), runtime_boot_id: "ef".repeat(16) }),
      ]),
    ),
  ).rejects.toThrow()
  await expect(readSnapshot(response(encoded, "ef".repeat(16)))).rejects.toThrow()
  await expect(readSnapshot(response([...encoded, first]))).rejects.toThrow()
})

test("errored snapshot stream preserves RuntimeApiPayloadError and releases its reader lock", async () => {
  const body = new ReadableStream<Uint8Array>({
    pull(controller) {
      controller.error(new TypeError("network reset"))
    },
  })
  const errored = new Response(body, {
    headers: { "content-type": "application/x-ndjson" },
  })

  try {
    await readSnapshot(errored)
    expect.unreachable("expected readSnapshot to reject")
  } catch (error) {
    expect(error).toBeInstanceOf(RuntimeApiPayloadError)
  }
  expect(body.locked).toBe(false)
})

test.each(["application/json", "application/x-ndjson"])(
  "aborts stalled %s snapshot bodies after headers and releases the reader lock",
  async (contentType) => {
    vi.useFakeTimers()
    try {
      const body = new ReadableStream<Uint8Array>({ start() {} })
      let requestSignal: AbortSignal | undefined
      vi.spyOn(globalThis, "fetch").mockImplementation(async (input, init) => {
        requestSignal = input instanceof Request ? input.signal : (init?.signal ?? undefined)
        return new Response(body, {
          status: 200,
          headers: {
            "content-type": contentType,
            [RUNTIME_BOOT_ID_HEADER]: "ab".repeat(16),
          },
        })
      })
      const client = createRuntimeApiClient({
        snapshot: "/api/v1/snapshot",
        events: "/api/v1/events",
      })
      let rejected = false
      const pending = client.fetchSnapshot().catch(() => {
        rejected = true
      })

      for (let i = 0; i < 10; i += 1) await Promise.resolve()
      vi.advanceTimersByTime(30_000)
      for (let i = 0; i < 10; i += 1) await Promise.resolve()

      expect(requestSignal?.aborted).toBe(true)
      expect(rejected).toBe(true)
      expect(body.locked).toBe(false)
      await pending
    } finally {
      vi.useRealTimers()
      vi.restoreAllMocks()
    }
  },
)

test.each([
  ["malformed", { "content-type": "application/json", [RUNTIME_BOOT_ID_HEADER]: "not-hex" }],
  ["missing", { "content-type": "application/json" }],
  [
    "malformed ndjson",
    { "content-type": "application/x-ndjson", [RUNTIME_BOOT_ID_HEADER]: "not-hex" },
  ],
  [
    "malformed csrf cookie",
    {
      "content-type": "application/json",
      [RUNTIME_BOOT_ID_HEADER]: "ab".repeat(16),
      "x-ma2a-csrf-cookie": "ma2a_session_1234",
    },
  ],
  [
    "malformed ndjson csrf cookie",
    {
      "content-type": "application/x-ndjson",
      [RUNTIME_BOOT_ID_HEADER]: "ab".repeat(16),
      "x-ma2a-csrf-cookie": "",
    },
  ],
])(
  "cancels and releases unread response body when snapshot headers are %s",
  async (_label, headers) => {
    let cancelled = false
    const body = new ReadableStream<Uint8Array>({
      start() {},
      cancel() {
        cancelled = true
      },
    })
    const response = new Response(body, { status: 200, headers })

    await expect(readSnapshot(response)).rejects.toThrow(RuntimeApiPayloadError)
    expect(cancelled).toBe(true)
    expect(body.locked).toBe(false)
  },
)

test("reads optional validated x-ma2a-csrf-cookie header from JSON and NDJSON snapshots", async () => {
  const bootId = "ab".repeat(16)
  const csrfCookie = "ma2a_csrf_11111111111111111111111111111111"
  const small = snapshot(5)
  const jsonResponse = Response.json(small, {
    headers: { [RUNTIME_BOOT_ID_HEADER]: bootId, "x-ma2a-csrf-cookie": csrfCookie },
  })
  expect(await readSnapshot(jsonResponse)).toEqual({ snapshot: small, bootId, csrfCookie })

  const large = largeSnapshot(7)
  const encoded = frames(large, 7, bootId)
  const encoder = new TextEncoder()
  const ndjsonResponse = new Response(encoder.encode(`${encoded.join("\n")}\n`), {
    headers: {
      "content-type": "application/x-ndjson",
      [RUNTIME_BOOT_ID_HEADER]: bootId,
      "x-ma2a-csrf-cookie": csrfCookie,
    },
  })
  expect(await readSnapshot(ndjsonResponse)).toEqual({ snapshot: large, bootId, csrfCookie })
})
