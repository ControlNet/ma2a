import "@testing-library/jest-dom/vitest"

import { act, render, screen, waitFor } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { afterEach, expect, test, vi } from "vitest"

import { App } from "./app"
import { MANY_RUNTIME_FIXTURE } from "./test/fixtures"
import { controllers, holdControllers } from "./test/runtime-controller-fakes"

afterEach(() => {
  controllers.length = 0
  vi.restoreAllMocks()
  Reflect.deleteProperty(document, "cookie")
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
