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
