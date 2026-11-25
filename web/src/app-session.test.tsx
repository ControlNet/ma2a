import "@testing-library/jest-dom/vitest"

import { act, render, screen } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { afterEach, expect, test, vi } from "vitest"

import { App } from "./app"
import type { RuntimeControllerCallbacks } from "./runtime-controller"
import { MANY_RUNTIME_FIXTURE } from "./test/fixtures"

// Each controller the console starts reports through these callbacks; the test plays the Runtime.
const controllers: RuntimeControllerCallbacks[] = []
vi.mock("./runtime-controller", () => ({
  RuntimeController: class {
    constructor(callbacks: RuntimeControllerCallbacks) {
      controllers.push(callbacks)
    }
    start(): Promise<void> {
      return Promise.resolve()
    }
    refresh(): Promise<void> {
      return Promise.resolve()
    }
    stop(): void {}
  },
}))

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
