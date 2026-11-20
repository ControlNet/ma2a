import { renderHook, waitFor } from "@testing-library/react"
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

  const { result, unmount } = renderHook(() => useServices(true, poll))
  await pause(60)

  // Many poll periods passed while the first read was still open: no second read started.
  expect(settle).toHaveLength(1)
  settle[0]?.(SERVICES_FIXTURE)
  await waitFor(() => expect(result.current.status).toBe("ready"))
  await waitFor(() => expect(settle).toHaveLength(2))
  unmount()
})
