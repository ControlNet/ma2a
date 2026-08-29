import { afterEach, expect, test, vi } from "vitest"

import { loginAndTouchSession } from "./web-auth"

afterEach(() => {
  vi.restoreAllMocks()
})

test("uses the returned CSRF token for an authenticated mutation without browser storage", async () => {
  // Given
  const requests: Request[] = []
  vi.spyOn(globalThis, "fetch").mockImplementation(async (input, init): Promise<Response> => {
    const request = new Request(input, init)
    requests.push(request)
    if (requests.length === 1) {
      return Response.json({ csrf_token: "ab".repeat(32) })
    }
    return new Response(null, { status: 204 })
  })
  const localStorageWrite = vi.spyOn(window.localStorage, "setItem")

  // When
  await loginAndTouchSession("browser-login-passphrase-9!")

  // Then
  expect(requests).toHaveLength(2)
  expect(new URL(requests[0]?.url ?? "https://invalid/").pathname).toBe("/api/v1/web/auth/login")
  expect(new URL(requests[1]?.url ?? "https://invalid/").pathname).toBe("/api/v1/web/session/touch")
  expect(requests[1]?.headers.get("x-csrf-token")).toBe("ab".repeat(32))
  expect(localStorageWrite).not.toHaveBeenCalled()
})
