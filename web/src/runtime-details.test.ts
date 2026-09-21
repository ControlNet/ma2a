import { describe, expect, test } from "vitest"

import type { RuntimeApiClient, RuntimeEventCallbacks } from "./api/client"
import { parseSpaceDetails, type SpaceDetails } from "./api/codec"
import { runtimeSnapshot } from "./api/test-fixtures"
import { RuntimeController } from "./runtime-controller"
import type { RuntimeViewData } from "./view-model"

// Synthetic API fixtures model delayed, failed and reordered network responses.
function detail(index: number, hash = "aa".repeat(32)): SpaceDetails {
  return {
    revision: 10,
    space: {
      space_id: index.toString(16).padStart(64, "0"),
      name: "Test Space",
      member_count: 1,
      generation: 1,
      chain_hash: hash,
      revoked_count: 0,
    },
    members: [{ endpoint_id: "bb".repeat(32), label: "peer", echo: true, relay_provider: false }],
  }
}

function deferred<T>() {
  let resolve: (value: T) => void = () => {
    throw new Error("resolver not installed")
  }
  let reject: (error: unknown) => void = () => {
    throw new Error("rejector not installed")
  }
  const promise = new Promise<T>((yes, no) => {
    resolve = yes
    reject = no
  })
  return { promise, resolve, reject }
}

async function flush(): Promise<void> {
  for (let turn = 0; turn < 12; turn += 1) await Promise.resolve()
}

function scenario(details: SpaceDetails[]) {
  let snapshot = { ...runtimeSnapshot(10), spaces: details.map((item) => item.space) }
  let latest: RuntimeViewData | undefined
  let events: RuntimeEventCallbacks | undefined
  const requests: { id: string; pending: ReturnType<typeof deferred<SpaceDetails>> }[] = []
  const errors: unknown[] = []
  const client: RuntimeApiClient = {
    fetchSnapshot: () => Promise.resolve(snapshot),
    fetchSpaceDetails: (id) => {
      const pending = deferred<SpaceDetails>()
      requests.push({ id, pending })
      return pending.promise
    },
    subscribe: (_revision, callbacks) => {
      events = callbacks
      return () => undefined
    },
  }
  const controller = new RuntimeController(
    {
      onRuntime: (runtime) => {
        latest = runtime
      },
      onSessionExpired: () => undefined,
      onError: (error) => errors.push(error),
    },
    client,
  )
  return {
    controller,
    requests,
    errors,
    latest: () => latest,
    events: () => events,
    update: (next: SpaceDetails[]) => {
      snapshot = {
        ...snapshot,
        revision: snapshot.revision + 1,
        spaces: next.map((item) => item.space),
      }
    },
  }
}

describe("Space detail loading", () => {
  test("limits concurrency and reuses unchanged chain heads across revisions", async () => {
    const details = Array.from({ length: 9 }, (_, index) => detail(index))
    const given = scenario(details)
    await given.controller.start()
    expect(given.requests).toHaveLength(4)
    expect(given.latest()?.spaces[0]?.members).toBeUndefined()
    for (let index = 0; index < details.length; index += 1) {
      const value = details[index]
      if (value === undefined) throw new Error("missing fixture")
      given.requests[index]?.pending.resolve(value)
      await flush()
      expect(given.requests.length - index - 1).toBeLessThanOrEqual(4)
    }
    expect(given.latest()?.spaces.every((space) => space.members?.length === 1)).toBe(true)
    given.update(details)
    await given.controller.refresh()
    expect(given.requests).toHaveLength(9)
    given.controller.stop()
  })

  test("discards an old head arriving after a new snapshot and clears removed Spaces", async () => {
    const before = detail(1)
    const after = detail(1, "cc".repeat(32))
    const given = scenario([before])
    await given.controller.start()
    given.update([after])
    await given.controller.refresh()
    given.requests[0]?.pending.resolve(before)
    await flush()
    expect(given.latest()?.spaces[0]?.members).toBeUndefined()
    given.requests[1]?.pending.resolve(after)
    await flush()
    expect(given.latest()?.spaces[0]?.members).toHaveLength(1)
    given.update([])
    await given.controller.refresh()
    expect(given.latest()?.spaces).toEqual([])
    given.controller.stop()
  })

  test("invalidates pending detail responses on disconnect and ignores responses after stop", async () => {
    const value = detail(1)
    const given = scenario([value])
    await given.controller.start()
    given.events()?.onDisconnect()
    await flush()
    given.requests[0]?.pending.resolve(value)
    await flush()
    expect(given.latest()?.spaces[0]?.members).toBeUndefined()
    given.controller.stop()
    const before = given.latest()
    given.requests[1]?.pending.resolve(value)
    await flush()
    expect(given.latest()).toBe(before)
  })

  test("keeps failed details distinct from an empty member set and retries on refresh", async () => {
    const value = detail(1)
    const given = scenario([value])
    await given.controller.start()
    given.requests[0]?.pending.reject(new Error("offline"))
    await flush()
    expect(given.requests).toHaveLength(1)
    expect(given.latest()?.spaces[0]?.members).toBeUndefined()
    expect(given.latest()?.spaces[0]?.membersError).toBe(true)
    await given.controller.refresh()
    given.requests[1]?.pending.resolve(value)
    await flush()
    expect(given.latest()?.spaces[0]?.members).toHaveLength(1)
    expect(given.latest()?.spaces[0]?.membersError).toBe(false)
    given.controller.stop()
  })

  test("rejects partial, duplicated, or foreign detail responses", async () => {
    const value = detail(1)
    expect(() => parseSpaceDetails({ ...value, members: [] })).toThrow()
    expect(() =>
      parseSpaceDetails({
        ...value,
        space: { ...value.space, member_count: 2 },
        members: [value.members[0], value.members[0]],
      }),
    ).toThrow()
    const given = scenario([value])
    await given.controller.start()
    given.requests[0]?.pending.resolve(detail(2))
    await flush()
    expect(given.latest()?.spaces[0]?.members).toBeUndefined()
    expect(given.latest()?.spaces[0]?.membersError).toBe(true)
    given.controller.stop()
  })
})
