import { renderHook, waitFor } from "@testing-library/react"
import { HTTPError } from "ky"
import { afterEach, expect, test, vi } from "vitest"

import { fetchServicesData, type ServicesData } from "../api/services"
import { SERVICES_FIXTURE } from "../test/services-fixture"
import { type ServicesPoll, useServices } from "./use-services"

const pause = (ms: number): Promise<void> => new Promise((resolve) => setTimeout(resolve, ms))
const NOOP = (): void => undefined
const FAST_FETCH_POLL: ServicesPoll = { load: fetchServicesData, everyMs: 5 }

afterEach(() => {
  vi.restoreAllMocks()
})

test("a slow read is never overlapped, so it cannot overwrite a newer one", async () => {
  const settle: Array<(data: ServicesData) => void> = []
  const poll: ServicesPoll = {
    load: () => new Promise<ServicesData>((resolve) => settle.push(resolve)),
    everyMs: 5,
  }

  const { result, unmount } = renderHook(() => useServices(true, NOOP, poll))
  await pause(60)

  // Many poll periods passed while the first read was still open: no second read started.
  expect(settle).toHaveLength(1)
  settle[0]?.(SERVICES_FIXTURE)
  await waitFor(() => expect(result.current.status).toBe("ready"))
  await waitFor(() => expect(settle).toHaveLength(2))
  unmount()
})

test("a read refused for a session that is gone signs out and stops polling", async () => {
  let reads = 0
  let expired = 0
  const request = new Request("http://localhost/api/v1/operations")
  const poll: ServicesPoll = {
    load: () => {
      reads += 1
      const options = {} as ConstructorParameters<typeof HTTPError>[2]
      return Promise.reject(new HTTPError(new Response(null, { status: 401 }), request, options))
    },
    everyMs: 5,
  }

  const onSessionExpired = (): void => {
    expired += 1
  }
  const { unmount } = renderHook(() => useServices(true, onSessionExpired, poll))
  await pause(60)

  expect(expired).toBe(1)
  expect(reads).toBe(1)
  unmount()
})

test("a fast 503 settles and aborts slow siblings before the next poll starts", async () => {
  let inFlight = 0
  let maxInFlight = 0
  let aborted = 0
  let overlaps = 0
  let batchStarts = 0
  vi.spyOn(globalThis, "fetch").mockImplementation((input, init) => {
    const request = new Request(input, init)
    if (new URL(request.url).pathname === "/api/v1/services") {
      if (inFlight > 0) overlaps += 1
      batchStarts += 1
      return Promise.resolve(new Response(null, { status: 503 }))
    }
    inFlight += 1
    maxInFlight = Math.max(maxInFlight, inFlight)
    return new Promise<Response>((resolve, reject) => {
      const timer = setTimeout(() => {
        inFlight -= 1
        resolve(Response.json({}))
      }, 30)
      request.signal.addEventListener(
        "abort",
        () => {
          clearTimeout(timer)
          inFlight -= 1
          aborted += 1
          reject(new DOMException("Aborted", "AbortError"))
        },
        { once: true },
      )
    })
  })

  const { result, unmount } = renderHook(() => useServices(true, NOOP, FAST_FETCH_POLL))
  await waitFor(() => expect(batchStarts).toBeGreaterThanOrEqual(3))

  expect(result.current.status).toBe("failed")
  expect(overlaps).toBe(0)
  expect(maxInFlight).toBe(4)
  expect(aborted).toBeGreaterThanOrEqual(8)
  unmount()
})

test("a fast 503 waits for delayed siblings that ignore abort before scheduling another poll", async () => {
  let inFlight = 0
  let overlaps = 0
  let batchStarts = 0
  vi.spyOn(globalThis, "fetch").mockImplementation((input, init) => {
    const request = new Request(input, init)
    if (new URL(request.url).pathname === "/api/v1/services") {
      if (inFlight > 0) overlaps += 1
      batchStarts += 1
      return Promise.resolve(new Response(null, { status: 503 }))
    }
    inFlight += 1
    return new Promise<Response>((resolve) => {
      setTimeout(() => {
        inFlight -= 1
        resolve(Response.json({}))
      }, 25)
    })
  })

  const { unmount } = renderHook(() => useServices(true, NOOP, FAST_FETCH_POLL))
  await waitFor(() => expect(batchStarts).toBeGreaterThanOrEqual(2))

  expect(overlaps).toBe(0)
  unmount()
})

test("an authentication failure during a partial Services poll aborts siblings and ends the session", async () => {
  let expired = 0
  let aborted = 0
  let starts = 0
  vi.spyOn(globalThis, "fetch").mockImplementation((input, init) => {
    const request = new Request(input, init)
    starts += 1
    if (new URL(request.url).pathname === "/api/v1/services") {
      return Promise.resolve(new Response(null, { status: 401 }))
    }
    return new Promise<Response>((_resolve, reject) => {
      request.signal.addEventListener(
        "abort",
        () => {
          aborted += 1
          reject(new DOMException("Aborted", "AbortError"))
        },
        { once: true },
      )
    })
  })

  const onSessionExpired = (): void => {
    expired += 1
  }
  const { unmount } = renderHook(() => useServices(true, onSessionExpired, FAST_FETCH_POLL))
  await waitFor(() => expect(expired).toBe(1))
  await pause(20)

  expect(aborted).toBe(4)
  expect(starts).toBe(5)
  unmount()
})
