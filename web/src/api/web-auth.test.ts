import { afterEach, expect, test, vi } from "vitest"

import { currentWebSession, loginAndTouchSession, logoutSession } from "./web-auth"

afterEach(() => {
  vi.restoreAllMocks()
  Reflect.deleteProperty(document, "cookie")
})

test("uses the login CSRF token through logout without browser storage", async () => {
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
  const sessionStorageWrite = vi.spyOn(window.sessionStorage, "setItem")

  // When
  const session = await loginAndTouchSession("browser-login-password-9!")
  await logoutSession(session)

  // Then
  expect(requests).toHaveLength(3)
  expect(new URL(requests[0]?.url ?? "https://invalid/").pathname).toBe("/api/v1/web/auth/login")
  expect(new URL(requests[1]?.url ?? "https://invalid/").pathname).toBe("/api/v1/web/session/touch")
  expect(requests[1]?.headers.get("x-csrf-token")).toBe("ab".repeat(32))
  expect(new URL(requests[2]?.url ?? "https://invalid/").pathname).toBe("/api/v1/web/auth/logout")
  expect(requests[2]?.headers.get("x-csrf-token")).toBe("ab".repeat(32))
  expect(session.csrfToken).toBe("ab".repeat(32))
  expect(localStorageWrite).not.toHaveBeenCalled()
  expect(sessionStorageWrite).not.toHaveBeenCalled()
})

test("restores the current session CSRF token from its strict host cookie", () => {
  // Given
  Object.defineProperty(document, "cookie", {
    configurable: true,
    value: `ma2a_csrf=${"cd".repeat(32)}`,
  })

  // When
  const session = currentWebSession()

  // Then
  expect(session).toEqual({ csrfToken: "cd".repeat(32) })
})
