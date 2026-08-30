import {
  createRuntimeApiClient,
  parseRuntimeEvent,
  parseRuntimeSnapshot,
  RuntimeApiPayloadError,
  type RuntimeSnapshot,
} from "./client"
import { runtimeSnapshot } from "./test-fixtures"

type FakeEventListener = (event: Event) => void

class FakeEventSource {
  static instances: FakeEventSource[] = []

  readonly listeners = new Map<string, FakeEventListener[]>()
  closed = false

  constructor(
    readonly url: string,
    readonly withCredentials: boolean,
  ) {
    FakeEventSource.instances.push(this)
  }

  addEventListener(type: string, listener: FakeEventListener): void {
    const listeners = this.listeners.get(type) ?? []
    listeners.push(listener)
    this.listeners.set(type, listeners)
  }

  dispatch(type: string, event: Event): void {
    for (const listener of this.listeners.get(type) ?? []) {
      listener(event)
    }
  }

  close(): void {
    this.closed = true
  }

  static latest(): FakeEventSource {
    const source = FakeEventSource.instances.at(-1)
    if (source === undefined) {
      throw new TypeError("No fake EventSource was constructed")
    }
    return source
  }
}

afterEach(() => {
  FakeEventSource.instances = []
})

describe("runtime API boundary", () => {
  test("parses the planned safe snapshot envelope", () => {
    const given = runtimeSnapshot(10)

    const when = parseRuntimeSnapshot(given)

    expect(when).toEqual(given)
  })

  test("rejects malformed snapshot revisions with a typed boundary error", () => {
    const given = { ...runtimeSnapshot(10), revision: -1 }

    const when = (): RuntimeSnapshot => parseRuntimeSnapshot(given)

    expect(when).toThrow(RuntimeApiPayloadError)
  })

  test("rejects malformed nested snapshot fields at the HTTP boundary", () => {
    const given = {
      ...runtimeSnapshot(10),
      endpoint: {
        endpoint_id: "00".repeat(32),
        runtime_version: "0.1.0",
        online: "yes",
      },
    }

    const when = (): RuntimeSnapshot => parseRuntimeSnapshot(given)

    expect(when).toThrow(RuntimeApiPayloadError)
  })

  test("parses a closed typed runtime event", () => {
    const given = { type: "spaces_changed", revision: 11, changed: { space_ids: [] } }

    const when = parseRuntimeEvent(given)

    expect(when).toEqual(given)
  })

  test("rejects unknown runtime event classes", () => {
    const given = { type: "unknown", revision: 11, changed: {} }

    const when = (): ReturnType<typeof parseRuntimeEvent> => parseRuntimeEvent(given)

    expect(when).toThrow(RuntimeApiPayloadError)
  })

  test("subscribes from the installed revision and closes malformed streams for recovery", () => {
    const payloadErrors: RuntimeApiPayloadError[] = []
    let recoveries = 0
    const given = createRuntimeApiClient(
      {
        snapshot: "/api/v1/snapshot",
        events: "/api/v1/events",
      },
      (url: string, init: EventSourceInit) =>
        new FakeEventSource(url, init.withCredentials ?? false),
    )

    given.subscribe(41, {
      onEvent: () => undefined,
      onResyncRequired: () => {
        recoveries += 1
      },
      onDisconnect: () => undefined,
      onPayloadError: (error) => payloadErrors.push(error),
    })
    const source = FakeEventSource.latest()
    source.dispatch("message", new MessageEvent("message", { data: "{" }))

    expect(source.url).toBe("/api/v1/events?since=41")
    expect(source.withCredentials).toBe(true)
    expect(payloadErrors).toHaveLength(1)
    expect(recoveries).toBe(1)
    expect(source.closed).toBe(true)
  })

  test("does not reclassify callback failures as malformed server payloads", () => {
    const callbackError = new TypeError("consumer failed")
    const payloadErrors: RuntimeApiPayloadError[] = []
    const given = createRuntimeApiClient(
      {
        snapshot: "/api/v1/snapshot",
        events: "/api/v1/events",
      },
      (url: string, init: EventSourceInit) =>
        new FakeEventSource(url, init.withCredentials ?? false),
    )
    given.subscribe(10, {
      onEvent: () => {
        throw callbackError
      },
      onResyncRequired: () => undefined,
      onDisconnect: () => undefined,
      onPayloadError: (error) => payloadErrors.push(error),
    })
    const source = FakeEventSource.latest()
    const event = new MessageEvent("message", {
      data: JSON.stringify({ type: "spaces_changed", revision: 11, changed: { space_ids: [] } }),
    })

    const when = (): void => source.dispatch("message", event)

    expect(when).toThrow(callbackError)
    expect(payloadErrors).toEqual([])
  })
})
