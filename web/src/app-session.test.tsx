import "@testing-library/jest-dom/vitest"

import { act, render, screen } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { afterEach, expect, test, vi } from "vitest"

import { App } from "./app"
import { RuntimeController, type RuntimeControllerCallbacks } from "./runtime-controller"
import { MANY_RUNTIME_FIXTURE } from "./test/fixtures"

// Each controller the console starts is held here instead of loading anything; the test plays
// the Runtime through its callbacks.
const controllers: RuntimeControllerCallbacks[] = []
function holdControllers(): void {
  vi.spyOn(RuntimeController.prototype, "start").mockImplementation(function (
    this: RuntimeController,
  ) {
    controllers.push((this as unknown as { callbacks: RuntimeControllerCallbacks }).callbacks)
    return Promise.resolve()
  })
  vi.spyOn(RuntimeController.prototype, "refresh").mockResolvedValue(undefined)
}

afterEach(() => {
  controllers.length = 0
  vi.restoreAllMocks()
  Reflect.deleteProperty(document, "cookie")
  window.history.replaceState(null, "", "/")
})

test("going back to a console page after signing out shows the sign-in screen", () => {
  window.history.replaceState(null, "", "/login")
  render(<App />)

  window.history.replaceState(null, "", "/settings")
  act(() => window.dispatchEvent(new window.PopStateEvent("popstate")))

  expect(screen.getByRole("heading", { name: "Sign in to this Runtime" })).toBeVisible()
})

test("signing in again does not show the last session's snapshot failure", async () => {
  Object.defineProperty(document, "cookie", {
    configurable: true,
    value: `ma2a_csrf=${"cd".repeat(32)}`,
  })
  vi.spyOn(globalThis, "fetch").mockImplementation(async (input, init): Promise<Response> => {
    const path = new URL(new Request(input, init).url).pathname
    return path === "/api/v1/web/auth/login"
      ? Response.json({ csrf_token: "ef".repeat(32) })
      : new Response(null, { status: 204 })
  })
  holdControllers()
  const user = userEvent.setup()
  render(<App />)
  act(() => controllers[0]?.onError(new Error("snapshot failed")))
  expect(screen.getByText("Runtime snapshot is unavailable")).toBeVisible()
  act(() => controllers[0]?.onSessionExpired())

  await user.type(screen.getByLabelText("Password"), "browser-login-password-9!")
  await user.click(screen.getByRole("button", { name: "Sign in" }))

  await screen.findByRole("main", { name: "Runtime content" })
  expect(controllers).toHaveLength(2)
  expect(screen.queryByText("Runtime snapshot is unavailable")).not.toBeInTheDocument()
})

test("a peer's inspector never shows another peer's sync status", async () => {
  Object.defineProperty(document, "cookie", {
    configurable: true,
    value: `ma2a_csrf=${"cd".repeat(32)}`,
  })
  let answer: (response: Response) => void = () => undefined
  vi.spyOn(globalThis, "fetch").mockImplementation(
    () => new Promise<Response>((resolve) => (answer = resolve)),
  )
  window.history.replaceState(null, "", "/endpoint")
  holdControllers()
  const user = userEvent.setup()
  const given = render(<App />)
  act(() => controllers[0]?.onRuntime(MANY_RUNTIME_FIXTURE))
  const [first, second] = Array.from(given.container.querySelectorAll<HTMLElement>(".peer"))
  if (first === undefined || second === undefined) throw new Error("fixture has two peers")

  await user.click(first)
  await user.click(screen.getByRole("button", { name: "Sync now" }))
  expect(screen.getByText(/Requesting a bounded control round/)).toBeVisible()
  await user.click(second)
  expect(screen.queryByText(/control round/)).not.toBeInTheDocument()
  await act(async () => answer(new Response(null, { status: 500 })))

  expect(screen.queryByText("The Runtime rejected the request.")).not.toBeInTheDocument()
})

test("signing in again forgets the last session's peer and its sync answer", async () => {
  Object.defineProperty(document, "cookie", {
    configurable: true,
    value: `ma2a_csrf=${"cd".repeat(32)}`,
  })
  let answer: (response: Response) => void = () => undefined
  vi.spyOn(globalThis, "fetch").mockImplementation((input, init): Promise<Response> => {
    const path = new URL(new Request(input, init).url).pathname
    if (path === "/api/v1/control-sync/trigger") {
      return new Promise<Response>((resolve) => (answer = resolve))
    }
    return Promise.resolve(
      path === "/api/v1/web/auth/login"
        ? Response.json({ csrf_token: "ef".repeat(32) })
        : new Response(null, { status: 204 }),
    )
  })
  window.history.replaceState(null, "", "/endpoint")
  holdControllers()
  const user = userEvent.setup()
  const given = render(<App />)
  act(() => controllers[0]?.onRuntime(MANY_RUNTIME_FIXTURE))
  const peer = (): HTMLElement => {
    const first = given.container.querySelector<HTMLElement>(".peer")
    if (first === null) throw new Error("fixture has a peer")
    return first
  }
  await user.click(peer())
  await user.click(screen.getByRole("button", { name: "Sync now" }))
  const pending = answer
  act(() => controllers[0]?.onSessionExpired())

  await user.type(screen.getByLabelText("Password"), "browser-login-password-9!")
  await user.click(screen.getByRole("button", { name: "Sign in" }))
  await screen.findByRole("main", { name: "Runtime content" })
  act(() => controllers[1]?.onRuntime(MANY_RUNTIME_FIXTURE))
  window.history.replaceState(null, "", "/endpoint")
  act(() => window.dispatchEvent(new window.PopStateEvent("popstate")))

  expect(screen.queryByRole("button", { name: "Sync now" })).not.toBeInTheDocument()
  await act(async () => pending(new Response(null, { status: 500 })))
  await user.click(peer())
  expect(screen.getByRole("button", { name: "Sync now" })).toBeVisible()
  expect(screen.queryByText(/control round|rejected the request/)).not.toBeInTheDocument()
})

test("the sign-in screen loads nothing for a session that may be gone", () => {
  Object.defineProperty(document, "cookie", {
    configurable: true,
    value: `ma2a_csrf=${"cd".repeat(32)}`,
  })
  holdControllers()
  window.history.replaceState(null, "", "/login")

  render(<App />)

  expect(screen.getByRole("heading", { name: "Sign in to this Runtime" })).toBeVisible()
  expect(controllers).toHaveLength(0)
})

test("signing in again after going back does not show the last load failure", async () => {
  Object.defineProperty(document, "cookie", {
    configurable: true,
    value: `ma2a_csrf=${"cd".repeat(32)}`,
  })
  vi.spyOn(globalThis, "fetch").mockImplementation(async (input, init): Promise<Response> => {
    const path = new URL(new Request(input, init).url).pathname
    return path === "/api/v1/web/auth/login"
      ? Response.json({ csrf_token: "ef".repeat(32) })
      : new Response(null, { status: 204 })
  })
  holdControllers()
  const user = userEvent.setup()
  render(<App />)
  act(() => controllers[0]?.onError(new Error("snapshot failed")))
  expect(screen.getByText("Runtime snapshot is unavailable")).toBeVisible()
  window.history.replaceState(null, "", "/login")
  act(() => window.dispatchEvent(new window.PopStateEvent("popstate")))

  await user.type(screen.getByLabelText("Password"), "browser-login-password-9!")
  await user.click(screen.getByRole("button", { name: "Sign in" }))

  await screen.findByRole("main", { name: "Runtime content" })
  expect(screen.queryByText("Runtime snapshot is unavailable")).not.toBeInTheDocument()
})

test.each([
  [401, true],
  [403, false],
])("a mutation answered %i signs out: %s", async (status, signsOut) => {
  Object.defineProperty(document, "cookie", {
    configurable: true,
    value: `ma2a_csrf=${"cd".repeat(32)}`,
  })
  vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response(null, { status }))
  window.history.replaceState(null, "", "/settings")
  holdControllers()
  const user = userEvent.setup()
  render(<App />)
  act(() => controllers[0]?.onRuntime(MANY_RUNTIME_FIXTURE))

  await user.click(screen.getByRole("button", { name: "Revoke all sessions" }))

  if (signsOut) {
    await screen.findByRole("heading", { name: "Sign in to this Runtime" })
  } else {
    await screen.findByText("Sessions could not be revoked.")
  }
})

// A request the last session sent can still be answered after the next sign-in; its answer
// must not end the session that is signed in now, nor log it out.
test.each([
  ["a command answered 401", "/endpoint", 401],
  ["a log out answered 204", "/settings", 204],
  ["a log out answered 403", "/settings", 403],
])("%s after signing in again keeps the new session", async (_, start, status) => {
  const cookie = (token: string): void => {
    Object.defineProperty(document, "cookie", { configurable: true, value: `ma2a_csrf=${token}` })
  }
  cookie("cd".repeat(32))
  const logouts: (string | null)[] = []
  let answer: (response: Response) => void = () => undefined
  vi.spyOn(globalThis, "fetch").mockImplementation((input, init): Promise<Response> => {
    const request = new Request(input, init)
    const path = new URL(request.url).pathname
    if (path === "/api/v1/web/auth/logout") logouts.push(request.headers.get("x-csrf-token"))
    if (path === "/api/v1/control-sync/trigger" || path === "/api/v1/web/auth/logout") {
      return new Promise<Response>((resolve) => (answer = resolve))
    }
    if (path === "/api/v1/web/auth/login") cookie("ef".repeat(32))
    return Promise.resolve(
      path === "/api/v1/web/auth/login"
        ? Response.json({ csrf_token: "ef".repeat(32) })
        : new Response(null, { status: 204 }),
    )
  })
  window.history.replaceState(null, "", start)
  holdControllers()
  const user = userEvent.setup()
  const given = render(<App />)
  act(() => controllers[0]?.onRuntime(MANY_RUNTIME_FIXTURE))
  if (start === "/endpoint") {
    const peer = given.container.querySelector<HTMLElement>(".peer")
    if (peer === null) throw new Error("fixture has a peer")
    await user.click(peer)
    await user.click(screen.getByRole("button", { name: "Sync now" }))
  } else {
    await user.click(screen.getByRole("button", { name: "Log out" }))
  }
  const pending = answer
  act(() => controllers[0]?.onSessionExpired())

  await user.type(screen.getByLabelText("Password"), "browser-login-password-9!")
  await user.click(screen.getByRole("button", { name: "Sign in" }))
  await screen.findByRole("main", { name: "Runtime content" })
  await act(async () => pending(new Response(null, { status })))

  expect(screen.getByRole("main", { name: "Runtime content" })).toBeVisible()
  expect(screen.queryByRole("heading", { name: "Sign in to this Runtime" })).not.toBeInTheDocument()
  expect(logouts).not.toContain("ef".repeat(32))
})
