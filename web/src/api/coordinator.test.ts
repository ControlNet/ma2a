import {
  RUNTIME_BOOT_ID_HEADER,
  RuntimeStateCoordinator,
  StaleRuntimeSnapshotError,
} from "./client"
import { readSnapshot } from "./snapshot-stream"
import {
  DEFAULT_BOOT_ID,
  deferredRuntimeSnapshot,
  runtimeSnapshot,
  stampedRuntimeSnapshot,
} from "./test-fixtures"

function snapshotResponse(format: "json" | "ndjson", revision: number, bootId: string): Response {
  const payload = runtimeSnapshot(revision)
  if (format === "json") {
    return Response.json(payload, { headers: { [RUNTIME_BOOT_ID_HEADER]: bootId } })
  }
  const bytes = new TextEncoder().encode(JSON.stringify(payload))
  const frame = JSON.stringify({
    type: "snapshot_fragment",
    revision,
    runtime_boot_id: bootId,
    index: 0,
    last: true,
    data_hex: Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join(""),
  })
  return new Response(`${frame}\n`, {
    headers: { "content-type": "application/x-ndjson", [RUNTIME_BOOT_ID_HEADER]: bootId },
  })
}

describe("runtime state coordinator", () => {
  test("resnapshots once on a gap and resumes only from the replacement revision", async () => {
    const replacement = deferredRuntimeSnapshot()
    const acceptedRevisions: number[] = []
    const appliedSnapshots: number[] = []
    let snapshotRequests = 0
    const given = new RuntimeStateCoordinator({
      fetchSnapshot: () => {
        snapshotRequests += 1
        return replacement.promise
      },
      onIncremental: (revision) => acceptedRevisions.push(revision),
      onSnapshot: (value) => appliedSnapshots.push(value.snapshot.revision),
      onRecoveryError: () => undefined,
    })
    given.install(stampedRuntimeSnapshot(10))

    const gapRecovery = given.receiveRevision(12)
    const discarded = await given.receiveRevision(13)
    replacement.resolve(stampedRuntimeSnapshot(20))
    await gapRecovery
    const accepted = await given.receiveRevision(21)

    expect(snapshotRequests).toBe(1)
    expect(discarded).toBe(false)
    expect(accepted).toBe(true)
    expect(appliedSnapshots).toEqual([10, 20])
    expect(acceptedRevisions).toEqual([21])
  })

  test("resnapshots once after disconnect and reports a failed recovery", async () => {
    const recoveryError = new TypeError("network unavailable")
    const observedErrors: unknown[] = []
    let snapshotRequests = 0
    const given = new RuntimeStateCoordinator({
      fetchSnapshot: () => {
        snapshotRequests += 1
        return Promise.reject(recoveryError)
      },
      onIncremental: () => undefined,
      onSnapshot: () => undefined,
      onRecoveryError: (error) => observedErrors.push(error),
    })
    given.install(stampedRuntimeSnapshot(10))

    await given.disconnect()
    const discarded = await given.receiveRevision(11)

    expect(snapshotRequests).toBe(1)
    expect(discarded).toBe(false)
    expect(observedErrors).toEqual([recoveryError])
    expect(given.currentState().kind).toBe("uncertain")
  })

  test("replaces state when the server explicitly requires resynchronization", async () => {
    const appliedSnapshots: number[] = []
    const given = new RuntimeStateCoordinator({
      fetchSnapshot: () => Promise.resolve(stampedRuntimeSnapshot(30)),
      onIncremental: () => undefined,
      onSnapshot: (value) => appliedSnapshots.push(value.snapshot.revision),
      onRecoveryError: () => undefined,
    })
    given.install(stampedRuntimeSnapshot(10))

    await given.resyncRequired()

    expect(appliedSnapshots).toEqual([10, 30])
    expect(given.currentState()).toEqual({ kind: "ready", revision: 30, bootId: DEFAULT_BOOT_ID })
  })

  test("treats snapshot invalidation as mandatory replacement instead of an incremental update", async () => {
    const acceptedRevisions: number[] = []
    const appliedSnapshots: number[] = []
    const given = new RuntimeStateCoordinator({
      fetchSnapshot: () => Promise.resolve(stampedRuntimeSnapshot(20)),
      onIncremental: (revision) => acceptedRevisions.push(revision),
      onSnapshot: (value) => appliedSnapshots.push(value.snapshot.revision),
      onRecoveryError: () => undefined,
    })
    given.install(stampedRuntimeSnapshot(10))

    await given.receiveEvent({ type: "snapshot_invalidated", revision: 11, changed: {} })

    expect(acceptedRevisions).toEqual([])
    expect(appliedSnapshots).toEqual([10, 20])
    expect(given.currentState()).toEqual({ kind: "ready", revision: 20, bootId: DEFAULT_BOOT_ID })
  })

  test("allows a later uncertainty signal to retry a failed replacement snapshot", async () => {
    const recoveryError = new TypeError("first request failed")
    let attempts = 0
    const given = new RuntimeStateCoordinator({
      fetchSnapshot: () => {
        attempts += 1
        return attempts === 1
          ? Promise.reject(recoveryError)
          : Promise.resolve(stampedRuntimeSnapshot(30))
      },
      onIncremental: () => undefined,
      onSnapshot: () => undefined,
      onRecoveryError: () => undefined,
    })
    given.install(stampedRuntimeSnapshot(10))

    await given.disconnect()
    await given.resyncRequired()

    expect(attempts).toBe(2)
    expect(given.currentState()).toEqual({ kind: "ready", revision: 30, bootId: DEFAULT_BOOT_ID })
  })

  test.each(["json", "ndjson"] as const)(
    "rejects a lower revision from the same boot and accepts one from a new boot (%s)",
    async (format) => {
      const bootA = "aa".repeat(16)
      const bootB = "bb".repeat(16)
      const responses = [snapshotResponse(format, 5, bootA), snapshotResponse(format, 5, bootB)]
      const appliedSnapshots: number[] = []
      const observedErrors: unknown[] = []
      const given = new RuntimeStateCoordinator({
        fetchSnapshot: () => {
          const next = responses.shift()
          if (next === undefined) throw new Error("unexpected fetch")
          return readSnapshot(next)
        },
        onIncremental: () => undefined,
        onSnapshot: (value) => appliedSnapshots.push(value.snapshot.revision),
        onRecoveryError: (error) => observedErrors.push(error),
      })
      given.install(await readSnapshot(snapshotResponse(format, 30, bootA)))

      await given.disconnect()
      expect(appliedSnapshots).toEqual([30])
      expect(observedErrors).toHaveLength(1)
      expect(given.currentState()).toEqual({
        kind: "uncertain",
        lastRevision: 30,
        lastBootId: bootA,
        reason: "disconnected",
        resnapshot: "required",
      })

      await given.resyncRequired()
      expect(appliedSnapshots).toEqual([30, 5])
      expect(given.currentState()).toEqual({ kind: "ready", revision: 5, bootId: bootB })
    },
  )

  test("resnapshots on view-only snapshot_invalidated at the same revision and keeps revision gaps for revision only", async () => {
    const applied: Array<readonly [number, number]> = []
    const given = new RuntimeStateCoordinator({
      fetchSnapshot: () => Promise.resolve(stampedRuntimeSnapshot(10, DEFAULT_BOOT_ID, 2)),
      onIncremental: () => undefined,
      onSnapshot: (value) => applied.push([value.snapshot.revision, value.snapshot.view_sequence]),
      onRecoveryError: () => undefined,
    })
    given.install(stampedRuntimeSnapshot(10, DEFAULT_BOOT_ID, 1))

    await given.receiveEvent({ type: "snapshot_invalidated", revision: 10, changed: {} })

    expect(applied).toEqual([
      [10, 1],
      [10, 2],
    ])
    expect(await given.receiveRevision(11)).toBe(true)
    expect(given.currentState()).toEqual({ kind: "ready", revision: 11, bootId: DEFAULT_BOOT_ID })
  })

  test("rejects a lower view_sequence at the same revision and boot, and accepts a new boot", async () => {
    const bootA = "aa".repeat(16)
    const bootB = "bb".repeat(16)
    const responses = [
      stampedRuntimeSnapshot(10, bootA, 2),
      stampedRuntimeSnapshot(10, bootA, 4),
      stampedRuntimeSnapshot(10, bootB, 1),
    ]
    const applied: Array<readonly [string, number, number]> = []
    const observedErrors: unknown[] = []
    const given = new RuntimeStateCoordinator({
      fetchSnapshot: () => {
        const next = responses.shift()
        if (next === undefined) throw new Error("unexpected fetch")
        return Promise.resolve(next)
      },
      onIncremental: () => undefined,
      onSnapshot: (value) =>
        applied.push([value.bootId, value.snapshot.revision, value.snapshot.view_sequence]),
      onRecoveryError: (error) => observedErrors.push(error),
    })
    given.install(stampedRuntimeSnapshot(10, bootA, 3))

    await given.disconnect()
    expect(applied).toEqual([[bootA, 10, 3]])
    expect(observedErrors).toHaveLength(1)
    expect(observedErrors[0]).toBeInstanceOf(StaleRuntimeSnapshotError)
    expect(given.currentState().kind).toBe("uncertain")

    await given.resyncRequired()
    expect(applied).toEqual([
      [bootA, 10, 3],
      [bootA, 10, 4],
    ])

    await given.resyncRequired()
    expect(applied).toEqual([
      [bootA, 10, 3],
      [bootA, 10, 4],
      [bootB, 10, 1],
    ])
    expect(given.currentState()).toEqual({ kind: "ready", revision: 10, bootId: bootB })
  })
})
