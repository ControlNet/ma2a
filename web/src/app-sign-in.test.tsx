import "@testing-library/jest-dom/vitest"

import { act, render, screen, waitFor } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { afterEach, expect, test, vi } from "vitest"
import { runtimeSnapshot } from "./api/test-fixtures"
import { App } from "./app"
import { MANY_RUNTIME_FIXTURE } from "./test/fixtures"
import { controllers, holdControllers } from "./test/runtime-controller-fakes"

afterEach(() => {
  controllers.length = 0
  vi.restoreAllMocks()
  Reflect.deleteProperty(document, "cookie")
  document.head.innerHTML = ""
  window.history.replaceState(null, "", "/")
})

// Going back to the sign-in screen while a log out is in flight, then signing in: the old
// session's late refusal arrives while the new session is still being set up, and must not log
// the new one out.
test("a late log out refusal during a sign-in after going back keeps the new session", async () => {
  const cookie = (token: string): void => {
    Object.defineProperty(document, "cookie", { configurable: true, value: `ma2a_csrf=${token}` })
  }
  cookie("cd".repeat(32))
  const logouts: (string | null)[] = []
  const answers = new Map<string, (response: Response) => void>()
  vi.spyOn(globalThis, "fetch").mockImplementation((input, init): Promise<Response> => {
    const request = new Request(input, init)
    const path = new URL(request.url).pathname
    if (path === "/api/v1/web/auth/login") {
      cookie("ef".repeat(32))
      return Promise.resolve(Response.json({ csrf_token: "ef".repeat(32) }))
    }
    if (path === "/api/v1/web/auth/logout") logouts.push(request.headers.get("x-csrf-token"))
    return new Promise<Response>((resolve) => answers.set(path, resolve))
  })
  window.history.replaceState(null, "", "/settings")
  holdControllers()
  const user = userEvent.setup()
  render(<App />)
  act(() => controllers[0]?.onRuntime(MANY_RUNTIME_FIXTURE))
  await user.click(screen.getByRole("button", { name: "Log out" }))
  const logout = answers.get("/api/v1/web/auth/logout")
  window.history.replaceState(null, "", "/login")
  act(() => window.dispatchEvent(new window.PopStateEvent("popstate")))

  await user.type(screen.getByLabelText("Password"), "browser-login-password-9!")
  await user.click(screen.getByRole("button", { name: "Sign in" }))
  await waitFor(() => expect(answers.has("/api/v1/web/session/touch")).toBe(true))
  await act(async () => logout?.(new Response(null, { status: 403 })))
  await act(async () =>
    answers.get("/api/v1/web/session/touch")?.(new Response(null, { status: 204 })),
  )

  await screen.findByRole("main", { name: "Runtime content" })
  expect(logouts).toEqual(["cd".repeat(32)])
})

test("a tab from Runtime A uses Runtime B's CSRF cookie after signing in to B", async () => {
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
  const mutationTokens: (string | null)[] = []
  vi.spyOn(globalThis, "fetch").mockImplementation(async (input, init): Promise<Response> => {
    const request = new Request(input, init)
    const path = new URL(request.url).pathname
    if (path === "/api/v1/web/auth/login") {
      return Response.json({ csrf_token: tokenB1, csrf_cookie: cookieB })
    }
    if (path === "/api/v1/control-sync/trigger") {
      mutationTokens.push(request.headers.get("x-csrf-token"))
      return Response.json({
        version: 1,
        request_id: null,
        revision: 1,
        result: { type: "control_sync_triggered", payload: { peer_endpoint_ids: [] } },
      })
    }
    return new Response(null, { status: 204 })
  })
  window.history.replaceState(null, "", "/endpoint")
  holdControllers()
  const user = userEvent.setup()
  const given = render(<App />)
  act(() => controllers[0]?.onSessionExpired())

  await user.type(screen.getByLabelText("Password"), "browser-login-password-9!")
  await user.click(screen.getByRole("button", { name: "Sign in" }))
  await screen.findByRole("main", { name: "Runtime content" })
  act(() => controllers[1]?.onRuntime(MANY_RUNTIME_FIXTURE))
  window.history.replaceState(null, "", "/endpoint")
  act(() => window.dispatchEvent(new window.PopStateEvent("popstate")))

  const peer = given.container.querySelector<HTMLElement>(".peer")
  if (peer === null) throw new Error("fixture has a peer")
  await user.click(peer)
  await user.click(screen.getByRole("button", { name: "Sync now" }))
  await screen.findByText("Control round requested.")

  // Another tab signs in to B and rotates B's CSRF cookie while A's cookie stays in the jar.
  jar.set(cookieB, tokenB2)
  await user.click(screen.getByRole("button", { name: "Sync now" }))

  expect(mutationTokens).toEqual([tokenB1, tokenB2])
})

test("a tab adopts Runtime B's CSRF cookie from snapshot after replacement without 401", async () => {
  const tokenA = "aa".repeat(32)
  const tokenB = "bb".repeat(32)
  const cookieA = "ma2a_csrf_11111111111111111111111111111111"
  const cookieB = "ma2a_csrf_22222222222222222222222222222222"
  const bootA = "11".repeat(16)
  const bootB = "22".repeat(16)
  document.head.innerHTML = `<meta name="ma2a-csrf-cookie" content="${cookieA}" />`
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

  type Listener = (event: Event) => void
  const eventSources: Array<{
    readonly listeners: Map<string, Listener[]>
    readonly close: () => void
  }> = []
  const originalEventSource = Object.getOwnPropertyDescriptor(globalThis, "EventSource")
  class StubEventSource {
    readonly listeners = new Map<string, Listener[]>()
    constructor() {
      eventSources.push(this)
    }
    addEventListener(type: string, listener: Listener): void {
      const list = this.listeners.get(type) ?? []
      list.push(listener)
      this.listeners.set(type, list)
    }
    close(): void {}
  }
  Object.defineProperty(globalThis, "EventSource", {
    configurable: true,
    value: StubEventSource,
  })

  const mutationTokens: (string | null)[] = []
  const logoutTokens: (string | null)[] = []
  let activeRuntime: "A" | "B" = "A"
  vi.spyOn(globalThis, "fetch").mockImplementation(async (input, init): Promise<Response> => {
    const request = new Request(input, init)
    const path = new URL(request.url).pathname
    if (path === "/api/v1/snapshot") {
      const base = runtimeSnapshot(1)
      const snapshot = { ...base, control_sync: { peer_endpoint_ids: ["44".repeat(32)] } }
      const [bootId, csrfCookie] = activeRuntime === "A" ? [bootA, cookieA] : [bootB, cookieB]
      return Response.json(snapshot, {
        headers: {
          "x-ma2a-runtime-boot-id": bootId,
          "x-ma2a-csrf-cookie": csrfCookie,
        },
      })
    }
    if (path === "/api/v1/control-sync/trigger") {
      const token = request.headers.get("x-csrf-token")
      mutationTokens.push(token)
      if (activeRuntime === "B" && token !== tokenB) {
        return new Response(null, { status: 403 })
      }
      return Response.json({
        version: 1,
        request_id: null,
        revision: 2,
        result: { type: "control_sync_triggered", payload: { peer_endpoint_ids: [] } },
      })
    }
    if (path === "/api/v1/web/auth/logout") {
      const token = request.headers.get("x-csrf-token")
      logoutTokens.push(token)
      if (activeRuntime === "B" && token !== tokenB) {
        return new Response(null, { status: 403 })
      }
      jar.delete(cookieB)
      return new Response(null, { status: 204 })
    }
    return new Response(null, { status: 204 })
  })

  try {
    window.history.replaceState(null, "", "/endpoint")
    const user = userEvent.setup()
    const given = render(<App />)
    await waitFor(() => expect(eventSources).toHaveLength(1))

    activeRuntime = "B"
    await act(async () => {
      for (const listener of eventSources[0]?.listeners.get("resync-required") ?? []) {
        listener(new Event("resync-required"))
      }
    })
    await waitFor(() => expect(eventSources).toHaveLength(2))

    const peer = given.container.querySelector<HTMLElement>(".peer")
    if (peer === null) throw new Error("snapshot has a peer")
    await user.click(peer)
    await user.click(screen.getByRole("button", { name: "Sync now" }))
    await screen.findByText("Control round requested.")

    await user.click(screen.getByRole("link", { name: "Settings" }))
    await user.click(screen.getByRole("button", { name: "Log out" }))
    await screen.findByRole("heading", { name: "Sign in to this Runtime" })

    expect(mutationTokens).toEqual([tokenB])
    expect(logoutTokens).toEqual([tokenA, tokenB])
  } finally {
    if (originalEventSource !== undefined) {
      Object.defineProperty(globalThis, "EventSource", originalEventSource)
    } else {
      Reflect.deleteProperty(globalThis, "EventSource")
    }
  }
})
