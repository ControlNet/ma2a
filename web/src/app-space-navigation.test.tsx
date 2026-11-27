import "@testing-library/jest-dom/vitest"

import { act, render, screen, waitFor, within } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { afterEach, expect, test, vi } from "vitest"

import { App } from "./app"
import { ONE_RUNTIME_FIXTURE } from "./test/fixtures"
import { controllers, holdControllers } from "./test/runtime-controller-fakes"

afterEach(() => {
  controllers.length = 0
  vi.restoreAllMocks()
  Reflect.deleteProperty(document, "cookie")
  window.sessionStorage.clear()
  window.history.replaceState(null, "", "/")
})

function setCsrfCookie(token = "cd".repeat(32)): void {
  Object.defineProperty(document, "cookie", { configurable: true, value: `ma2a_csrf=${token}` })
}

function spaceCreatedResponse(requestId: string, name: string, index: number): Response {
  return Response.json({
    version: 1,
    request_id: requestId,
    revision: 12 + index,
    result: {
      type: "space_created",
      payload: { space_id: index.toString(16).padStart(64, "0"), name, member_count: 1 },
    },
  })
}

// C06 (Back/Forward within one session): controller replacement must share the session's
// pending-space tracker so a late Alpha completion never erases Beta's unresolved record.
test("late Alpha success after Back/Forward keeps Beta's pending request ID across reload", async () => {
  setCsrfCookie()
  const idsByName: { Alpha: string[]; Beta: string[] } = { Alpha: [], Beta: [] }
  const committed = new Map<string, string>()
  let releaseAlpha: ((response: Response) => void) | undefined
  let dropBetaOnce = true
  vi.spyOn(globalThis, "fetch").mockImplementation(async (input, init): Promise<Response> => {
    const command = (await new Request(input, init).json()) as {
      request_id: string
      name: "Alpha" | "Beta"
    }
    idsByName[command.name].push(command.request_id)
    if (!committed.has(command.request_id)) committed.set(command.request_id, command.name)
    if (command.name === "Alpha") {
      return new Promise<Response>((resolve) => {
        releaseAlpha = resolve
      })
    }
    if (dropBetaOnce) {
      dropBetaOnce = false
      throw new TypeError("Failed to fetch")
    }
    return spaceCreatedResponse(command.request_id, command.name, committed.size)
  })
  window.history.replaceState(null, "", "/spaces")
  holdControllers()
  const user = userEvent.setup()

  const first = render(<App />)
  act(() => controllers[0]?.onRuntime(ONE_RUNTIME_FIXTURE))
  const alphaForm = within(screen.getByRole("form", { name: "Create a Space" }))
  await user.type(alphaForm.getByLabelText("Name"), "Alpha")
  await user.click(alphaForm.getByRole("button", { name: "Create" }))
  await waitFor(() => expect(releaseAlpha).toBeDefined())

  // Navigate Back out of the console, then Forward back into /spaces in the same session.
  window.history.replaceState(null, "", "/login")
  act(() => window.dispatchEvent(new window.PopStateEvent("popstate")))
  window.history.replaceState(null, "", "/spaces")
  act(() => window.dispatchEvent(new window.PopStateEvent("popstate")))
  await waitFor(() => expect(controllers.length).toBeGreaterThanOrEqual(2))
  act(() => controllers[1]?.onRuntime(ONE_RUNTIME_FIXTURE))

  const betaForm = within(screen.getByRole("form", { name: "Create a Space" }))
  await user.clear(betaForm.getByLabelText("Name"))
  await user.type(betaForm.getByLabelText("Name"), "Beta")
  await user.click(betaForm.getByRole("button", { name: "Create" }))
  expect(await screen.findByText(/Unresolved request for “Beta”/)).toBeInTheDocument()

  // Release Alpha's delayed success; Beta must stay unresolved in UI and storage.
  await act(async () => {
    const alphaId = idsByName.Alpha[0] ?? ""
    releaseAlpha?.(spaceCreatedResponse(alphaId, "Alpha", 1))
  })
  expect(screen.getByText(/Unresolved request for “Beta”/)).toBeInTheDocument()
  first.unmount()

  // Reload and retry Beta; it must reuse Beta's original request ID.
  render(<App />)
  act(() => controllers[2]?.onRuntime(ONE_RUNTIME_FIXTURE))
  expect(await screen.findByText(/Unresolved request for “Beta”/)).toBeInTheDocument()
  await user.click(screen.getByRole("button", { name: "Retry pending creation" }))
  const reloadedForm = within(screen.getByRole("form", { name: "Create a Space" }))
  expect(await reloadedForm.findByText("Space created by the Runtime.")).toBeInTheDocument()
  expect(idsByName.Beta).toHaveLength(2)
  expect(idsByName.Beta[1]).toBe(idsByName.Beta[0])
})

// C06 (re-login in same tab): a late response from an ended session must not touch the new
// session's storage or pending-space state.
test("late Alpha success from a previous session never erases the new session's Beta", async () => {
  setCsrfCookie("cd".repeat(32))
  const idsByName: { Alpha: string[]; Beta: string[] } = { Alpha: [], Beta: [] }
  let releaseAlpha: ((response: Response) => void) | undefined
  let dropBetaOnce = true
  vi.spyOn(globalThis, "fetch").mockImplementation(async (input, init): Promise<Response> => {
    const request = new Request(input, init)
    const path = new URL(request.url).pathname
    if (path === "/api/v1/web/auth/login") {
      setCsrfCookie("ef".repeat(32))
      return Response.json({ csrf_token: "ef".repeat(32) })
    }
    if (path === "/api/v1/web/session/touch") return new Response(null, { status: 204 })
    const command = (await request.json()) as { request_id: string; name: "Alpha" | "Beta" }
    idsByName[command.name].push(command.request_id)
    if (command.name === "Alpha") {
      return new Promise<Response>((resolve) => {
        releaseAlpha = resolve
      })
    }
    if (dropBetaOnce) {
      dropBetaOnce = false
      throw new TypeError("Failed to fetch")
    }
    return spaceCreatedResponse(command.request_id, command.name, 2)
  })
  window.history.replaceState(null, "", "/spaces")
  holdControllers()
  const user = userEvent.setup()

  const first = render(<App />)
  act(() => controllers[0]?.onRuntime(ONE_RUNTIME_FIXTURE))
  const alphaForm = within(screen.getByRole("form", { name: "Create a Space" }))
  await user.type(alphaForm.getByLabelText("Name"), "Alpha")
  await user.click(alphaForm.getByRole("button", { name: "Create" }))
  await waitFor(() => expect(releaseAlpha).toBeDefined())

  // Go to Login and sign in as a fresh session against the same Runtime Endpoint.
  window.history.replaceState(null, "", "/login")
  act(() => window.dispatchEvent(new window.PopStateEvent("popstate")))
  await user.type(screen.getByLabelText("Password"), "browser-login-password-9!")
  await user.click(screen.getByRole("button", { name: "Sign in" }))
  await waitFor(() => expect(controllers.length).toBeGreaterThanOrEqual(2))
  act(() => controllers[1]?.onRuntime(ONE_RUNTIME_FIXTURE))

  await user.click(screen.getByRole("link", { name: "Spaces" }))
  const betaForm = within(screen.getByRole("form", { name: "Create a Space" }))
  await user.type(betaForm.getByLabelText("Name"), "Beta")
  await user.click(betaForm.getByRole("button", { name: "Create" }))
  expect(await screen.findByText(/Unresolved request for “Beta”/)).toBeInTheDocument()

  // Release the old session's Alpha response; Beta in the new session must remain intact.
  await act(async () => {
    const alphaId = idsByName.Alpha[0] ?? ""
    releaseAlpha?.(spaceCreatedResponse(alphaId, "Alpha", 1))
  })
  expect(screen.getByText(/Unresolved request for “Beta”/)).toBeInTheDocument()
  first.unmount()

  window.history.replaceState(null, "", "/spaces")
  render(<App />)
  act(() => controllers[2]?.onRuntime(ONE_RUNTIME_FIXTURE))
  expect(await screen.findByText(/Unresolved request for “Beta”/)).toBeInTheDocument()
  await user.click(screen.getByRole("button", { name: "Retry pending creation" }))
  const reloadedForm = within(screen.getByRole("form", { name: "Create a Space" }))
  expect(await reloadedForm.findByText("Space created by the Runtime.")).toBeInTheDocument()
  expect(idsByName.Beta).toHaveLength(2)
  expect(idsByName.Beta[1]).toBe(idsByName.Beta[0])
})
