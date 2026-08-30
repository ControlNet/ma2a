import { RuntimeStateCoordinator } from "./client"
import { deferredRuntimeSnapshot, runtimeSnapshot } from "./test-fixtures"

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
      onSnapshot: (value) => appliedSnapshots.push(value.revision),
      onRecoveryError: () => undefined,
    })
    given.install(runtimeSnapshot(10))

    const gapRecovery = given.receiveRevision(12)
    const discarded = await given.receiveRevision(13)
    replacement.resolve(runtimeSnapshot(20))
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
    given.install(runtimeSnapshot(10))

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
      fetchSnapshot: () => Promise.resolve(runtimeSnapshot(30)),
      onIncremental: () => undefined,
      onSnapshot: (value) => appliedSnapshots.push(value.revision),
      onRecoveryError: () => undefined,
    })
    given.install(runtimeSnapshot(10))

    await given.resyncRequired()

    expect(appliedSnapshots).toEqual([10, 30])
    expect(given.currentState()).toEqual({ kind: "ready", revision: 30 })
  })

  test("treats snapshot invalidation as mandatory replacement instead of an incremental update", async () => {
    const acceptedRevisions: number[] = []
    const appliedSnapshots: number[] = []
    const given = new RuntimeStateCoordinator({
      fetchSnapshot: () => Promise.resolve(runtimeSnapshot(20)),
      onIncremental: (revision) => acceptedRevisions.push(revision),
      onSnapshot: (value) => appliedSnapshots.push(value.revision),
      onRecoveryError: () => undefined,
    })
    given.install(runtimeSnapshot(10))

    await given.receiveEvent({ type: "snapshot_invalidated", revision: 11, changed: {} })

    expect(acceptedRevisions).toEqual([])
    expect(appliedSnapshots).toEqual([10, 20])
    expect(given.currentState()).toEqual({ kind: "ready", revision: 20 })
  })

  test("allows a later uncertainty signal to retry a failed replacement snapshot", async () => {
    const recoveryError = new TypeError("first request failed")
    let attempts = 0
    const given = new RuntimeStateCoordinator({
      fetchSnapshot: () => {
        attempts += 1
        return attempts === 1 ? Promise.reject(recoveryError) : Promise.resolve(runtimeSnapshot(30))
      },
      onIncremental: () => undefined,
      onSnapshot: () => undefined,
      onRecoveryError: () => undefined,
    })
    given.install(runtimeSnapshot(10))

    await given.disconnect()
    await given.resyncRequired()

    expect(attempts).toBe(2)
    expect(given.currentState()).toEqual({ kind: "ready", revision: 30 })
  })

  test("rejects a replacement snapshot older than the last authoritative revision", async () => {
    const appliedSnapshots: number[] = []
    const observedErrors: unknown[] = []
    const given = new RuntimeStateCoordinator({
      fetchSnapshot: () => Promise.resolve(runtimeSnapshot(9)),
      onIncremental: () => undefined,
      onSnapshot: (value) => appliedSnapshots.push(value.revision),
      onRecoveryError: (error) => observedErrors.push(error),
    })
    given.install(runtimeSnapshot(10))

    await given.disconnect()

    expect(appliedSnapshots).toEqual([10])
    expect(observedErrors).toHaveLength(1)
    expect(given.currentState()).toEqual({
      kind: "uncertain",
      lastRevision: 10,
      reason: "disconnected",
      resnapshot: "required",
    })
  })
})
