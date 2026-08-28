import {
  parseRuntimeSnapshot,
  RuntimeApiPayloadError,
  type RuntimeSnapshot,
  RuntimeStateCoordinator,
} from "./client"

function snapshot(revision: number): RuntimeSnapshot {
  return {
    revision,
    endpoint: null,
    spaces: null,
    control_sync: null,
    relay_candidates: null,
    observed_relay_state: null,
    reachability: null,
    recent_echo_summary: null,
    ui_auth: null,
  }
}

function deferredSnapshot(): {
  readonly promise: Promise<RuntimeSnapshot>
  readonly resolve: (value: RuntimeSnapshot) => void
} {
  let resolveSnapshot: ((value: RuntimeSnapshot) => void) | undefined
  const promise = new Promise<RuntimeSnapshot>((resolve) => {
    resolveSnapshot = resolve
  })
  if (resolveSnapshot === undefined) {
    throw new TypeError("Deferred snapshot resolver was not initialized")
  }
  return { promise, resolve: resolveSnapshot }
}

describe("runtime API boundary", () => {
  test("parses the planned safe snapshot envelope", () => {
    const given = snapshot(10)

    const when = parseRuntimeSnapshot(given)

    expect(when).toEqual(given)
  })

  test("rejects malformed snapshot revisions with a typed boundary error", () => {
    const given = { ...snapshot(10), revision: -1 }

    const when = (): RuntimeSnapshot => parseRuntimeSnapshot(given)

    expect(when).toThrow(RuntimeApiPayloadError)
  })
})

describe("runtime state coordinator", () => {
  test("resnapshots once on a gap and resumes only from the replacement revision", async () => {
    const replacement = deferredSnapshot()
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
    given.install(snapshot(10))

    const gapRecovery = given.receiveRevision(12)
    const discarded = await given.receiveRevision(13)
    replacement.resolve(snapshot(20))
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
    given.install(snapshot(10))

    await given.disconnect()
    const discarded = await given.receiveRevision(11)

    expect(snapshotRequests).toBe(1)
    expect(discarded).toBe(false)
    expect(observedErrors).toEqual([recoveryError])
    expect(given.currentState().kind).toBe("uncertain")
  })
})
