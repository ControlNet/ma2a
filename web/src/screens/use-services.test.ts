import { renderHook, waitFor } from "@testing-library/react"
import { HTTPError } from "ky"
import { expect, test } from "vitest"

import type { ServicesData } from "../api/services"
import { SERVICES_FIXTURE } from "../test/services-fixture"
import { type ServicesPoll, useServices } from "./use-services"

const pause = (ms: number): Promise<void> => new Promise((resolve) => setTimeout(resolve, ms))

test("a slow read is never overlapped, so it cannot overwrite a newer one", async () => {
  const settle: Array<(data: ServicesData) => void> = []
  const poll: ServicesPoll = {
    load: () => new Promise<ServicesData>((resolve) => settle.push(resolve)),
    everyMs: 5,
  }

  const { result, unmount } = renderHook(() => useServices(true, () => undefined, poll))
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

  const { unmount } = renderHook(() => useServices(true, () => (expired += 1), poll))
  await pause(60)

  expect(expired).toBe(1)
  expect(reads).toBe(1)
  unmount()
})
