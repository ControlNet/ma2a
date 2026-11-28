import { afterEach, expect, test, vi } from "vitest"

import { currentWebSession, loginAndTouchSession, logoutSession, sessionIsGone } from "./web-auth"

afterEach(() => {
  vi.restoreAllMocks()
  Reflect.deleteProperty(document, "cookie")
  document.head.innerHTML = ""
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

test("logs out the browser's current session when another tab replaced it", async () => {
  // Given: this page holds the first session's token; the cookie now carries a newer session.
  Object.defineProperty(document, "cookie", {
    configurable: true,
    value: `ma2a_csrf=${"cd".repeat(32)}`,
  })
  const tokens: (string | null)[] = []
  vi.spyOn(globalThis, "fetch").mockImplementation(async (input, init): Promise<Response> => {
    const token = new Request(input, init).headers.get("x-csrf-token")
    tokens.push(token)
    return new Response(null, { status: token === "cd".repeat(32) ? 204 : 403 })
  })

  // When
  await logoutSession({ csrfToken: "ab".repeat(32) })

  // Then: the stale token is refused and the current session is logged out.
  expect(tokens).toEqual(["ab".repeat(32), "cd".repeat(32)])
})

test("reports a refused logout when the cookie carries the same token", async () => {
  // Given: the Runtime refuses this token, and the cookie offers no other one.
  Object.defineProperty(document, "cookie", {
    configurable: true,
    value: `ma2a_csrf=${"ab".repeat(32)}`,
  })
  const fetch = vi
    .spyOn(globalThis, "fetch")
    .mockImplementation(async (): Promise<Response> => new Response(null, { status: 403 }))

  // When / Then: no retry, and the refusal is reported to the caller.
  await expect(logoutSession({ csrfToken: "ab".repeat(32) })).rejects.toThrow(
    "The Runtime did not accept",
  )
  expect(fetch).toHaveBeenCalledTimes(1)
})

test("signs the page out only when the Runtime reports the session gone", async () => {
  // Given
  Object.defineProperty(document, "cookie", {
    configurable: true,
    value: `ma2a_csrf=${"ab".repeat(32)}`,
  })
  const answer = { status: 403 }
  vi.spyOn(globalThis, "fetch").mockImplementation(
    async (): Promise<Response> => new Response(null, answer),
  )

  // When
  const refused = await logoutSession({ csrfToken: "ab".repeat(32) }).catch((error) => error)
  answer.status = 401
  const gone = await logoutSession({ csrfToken: "ab".repeat(32) }).catch((error) => error)

  // Then: a 403 keeps the live session; only a 401 lets the page sign out.
  expect(sessionIsGone(refused)).toBe(false)
  expect(sessionIsGone(gone)).toBe(true)
  expect(sessionIsGone(new Error("offline"))).toBe(false)
})

test("selects this Runtime's scoped CSRF cookie when another Runtime shares the host", () => {
  const tokenA = "aa".repeat(32)
  const tokenB = "bb".repeat(32)
  const meta = document.createElement("meta")
  meta.name = "ma2a-csrf-cookie"
  meta.content = "ma2a_csrf_11111111111111111111111111111111"
  document.head.appendChild(meta)
  Object.defineProperty(document, "cookie", {
    configurable: true,
    value: [
      `ma2a_csrf_11111111111111111111111111111111=${tokenA}`,
      `ma2a_csrf_22222222222222222222222222222222=${tokenB}`,
    ].join("; "),
  })

  const session = currentWebSession()

  expect(session).toEqual({ csrfToken: tokenA })
})

test("logging out one Runtime in a shared host cookie jar keeps the other Runtime signed in", async () => {
  const tokenA = "aa".repeat(32)
  const tokenB = "bb".repeat(32)
  const cookieA = "ma2a_csrf_11111111111111111111111111111111"
  const cookieB = "ma2a_csrf_22222222222222222222222222222222"
  const jar = new Map<string, string>([
    [cookieA, tokenA],
    [cookieB, tokenB],
  ])
  Object.defineProperty(document, "cookie", {
    configurable: true,
    get: () =>
      Array.from(jar.entries())
        .map(([name, value]) => `${name}=${value}`)
        .join("; "),
  })
  const setMeta = (name: string): void => {
    document.head.innerHTML = `<meta name="ma2a-csrf-cookie" content="${name}" />`
  }
  const logoutTokens: (string | null)[] = []
  vi.spyOn(globalThis, "fetch").mockImplementation(async (input, init): Promise<Response> => {
    const request = new Request(input, init)
    logoutTokens.push(request.headers.get("x-csrf-token"))
    jar.delete(cookieB)
    return new Response(null, { status: 204 })
  })

  setMeta(cookieB)
  const sessionB = currentWebSession()
  if (sessionB === undefined) throw new Error("expected Runtime B session")
  await logoutSession(sessionB)

  setMeta(cookieA)
  expect(logoutTokens).toEqual([tokenB])
  expect(currentWebSession()).toEqual({ csrfToken: tokenA })
})

test("uses the login response CSRF cookie name for later reads and logout retries", async () => {
  const tokenA = "aa".repeat(32)
  const tokenB1 = "bb".repeat(32)
  const tokenB2 = "cc".repeat(32)
  const cookieA = "ma2a_csrf_11111111111111111111111111111111"
  const cookieB = "ma2a_csrf_22222222222222222222222222222222"
  document.head.innerHTML = `<meta name="ma2a-csrf-cookie" content="${cookieA}" />`
  const jar = new Map<string, string>([
    [cookieA, tokenA],
    [cookieB, tokenB1],
  ])
  Object.defineProperty(document, "cookie", {
    configurable: true,
    get: () =>
      Array.from(jar.entries())
        .map(([name, value]) => `${name}=${value}`)
        .join("; "),
  })
  const logoutTokens: (string | null)[] = []
  vi.spyOn(globalThis, "fetch").mockImplementation(async (input, init): Promise<Response> => {
    const request = new Request(input, init)
    const path = new URL(request.url).pathname
    if (path === "/api/v1/web/auth/login") {
      return Response.json({ csrf_token: tokenB1, csrf_cookie: cookieB })
    }
    if (path === "/api/v1/web/auth/logout") {
      const token = request.headers.get("x-csrf-token")
      logoutTokens.push(token)
      return new Response(null, { status: token === tokenB2 ? 204 : 403 })
    }
    return new Response(null, { status: 204 })
  })

  const session = await loginAndTouchSession("browser-login-password-9!")
  jar.set(cookieB, tokenB2)

  expect(currentWebSession()).toEqual({ csrfToken: tokenB2 })
  await logoutSession(session)
  expect(logoutTokens).toEqual([tokenB1, tokenB2])
})
