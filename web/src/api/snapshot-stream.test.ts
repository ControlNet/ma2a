import { readSnapshot } from "./snapshot-stream"
import { runtimeSnapshot as snapshot } from "./test-fixtures"

function frames(payload: unknown): string[] {
  const bytes = new TextEncoder().encode(JSON.stringify(payload))
  const result: string[] = []
  for (let offset = 0; offset < bytes.length; offset += 16_384) {
    const chunk = bytes.subarray(offset, offset + 16_384)
    result.push(
      JSON.stringify({
        type: "snapshot_fragment",
        revision: 7,
        runtime_boot_id: "ab".repeat(16),
        index: offset / 16_384,
        last: offset + chunk.length === bytes.length,
        data_hex: Array.from(chunk, (byte) => byte.toString(16).padStart(2, "0")).join(""),
      }),
    )
  }
  return result
}

function response(lines: readonly string[]): Response {
  const encoder = new TextEncoder()
  const wire = encoder.encode(`${lines.join("\n")}\n`)
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
    { headers: { "content-type": "application/x-ndjson" } },
  )
}

function largeSnapshot() {
  const base = snapshot(7)
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
  const encoded = frames(expected)
  expect(encoded.length).toBeGreaterThan(1)
  expect(encoded.every((frame) => frame.length < 33_000)).toBe(true)
  expect(await readSnapshot(response(encoded))).toEqual(expected)
})

test("incomplete, reordered, mixed-revision and extra frames never publish a snapshot", async () => {
  const encoded = frames(largeSnapshot())
  const first = encoded[0]
  const second = encoded[1]
  if (first === undefined || second === undefined) throw new Error("missing fixture frames")
  await expect(readSnapshot(response(encoded.slice(0, -1)))).rejects.toThrow()
  await expect(readSnapshot(response([second, first]))).rejects.toThrow()
  await expect(
    readSnapshot(response([first, JSON.stringify({ ...JSON.parse(second), revision: 8 })])),
  ).rejects.toThrow()
  await expect(readSnapshot(response([...encoded, first]))).rejects.toThrow()
})
