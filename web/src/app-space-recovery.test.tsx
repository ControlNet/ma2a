import "@testing-library/jest-dom/vitest"

import { act, render, screen, within } from "@testing-library/react"
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

function stubSpaceServer(dropFirst: boolean) {
  const observedIds: string[] = []
  const committed = new Map<string, { space_id: string; name: string; member_count: number }>()
  let shouldDrop = dropFirst
  vi.spyOn(globalThis, "fetch").mockImplementation(async (input, init): Promise<Response> => {
    const request = new Request(input, init)
    const path = new URL(request.url).pathname
    if (path === "/api/v1/web/auth/login") return Response.json({ csrf_token: "ef".repeat(32) })
    if (path === "/api/v1/web/session/touch") return new Response(null, { status: 204 })
    const command = (await request.json()) as { request_id: string; name: string }
    observedIds.push(command.request_id)
    let space = committed.get(command.request_id)
    if (space === undefined) {
      space = {
        space_id: (committed.size + 1).toString(16).padStart(64, "0"),
        name: command.name,
        member_count: 1,
      }
      committed.set(command.request_id, space)
    }
    if (shouldDrop) {
      shouldDrop = false
      throw new TypeError("Failed to fetch")
    }
    return Response.json({
      version: 1,
      request_id: command.request_id,
      revision: 12,
      result: { type: "space_created", payload: space },
    })
  })
  return { observedIds, committed }
}

function setCsrfCookie(token = "cd".repeat(32)): void {
  Object.defineProperty(document, "cookie", { configurable: true, value: `ma2a_csrf=${token}` })
}

// A page reload recreates App and its mutation client. Retrying an unresolved Space creation
// after reload (either by submitting the same name or via the explicit retry button) must reuse
// the original request ID so the server creates only one Space.
test.each(["form", "button"] as const)(
  "reloading the console after a lost Space creation answer reuses the original request ID (%s)",
  async (method) => {
    setCsrfCookie()
    const server = stubSpaceServer(true)
    window.history.replaceState(null, "", "/spaces")
    holdControllers()
    const user = userEvent.setup()

    const first = render(<App />)
    act(() => controllers[0]?.onRuntime(ONE_RUNTIME_FIXTURE))
    const firstForm = within(screen.getByRole("form", { name: "Create a Space" }))
    await user.type(firstForm.getByLabelText("Name"), "Staging")
    await user.click(firstForm.getByRole("button", { name: "Create" }))
    expect(await firstForm.findByText(/may already exist.*same name/)).toBeInTheDocument()
    first.unmount()

    // Simulate reloading the same authenticated tab.
    render(<App />)
    act(() => controllers[1]?.onRuntime(ONE_RUNTIME_FIXTURE))
    expect(await screen.findByText(/Unresolved request for “Staging”/)).toBeInTheDocument()
    const reloadedForm = within(screen.getByRole("form", { name: "Create a Space" }))
    if (method === "button") {
      await user.click(screen.getByRole("button", { name: "Retry pending creation" }))
    } else {
      await user.type(reloadedForm.getByLabelText("Name"), "Staging")
      await user.click(reloadedForm.getByRole("button", { name: "Create" }))
    }
    expect(await reloadedForm.findByText("Space created by the Runtime.")).toBeInTheDocument()

    expect(screen.queryByText(/Unresolved request for/)).not.toBeInTheDocument()
    expect(server.observedIds).toHaveLength(2)
    expect(server.observedIds[1]).toBe(server.observedIds[0])
    expect(server.committed.size).toBe(1)
  },
)

// Starting a new creation clears the unresolved record so submitting the same name uses a new ID.
test("discarding an unresolved Space creation after reload lets the same name start a new Space", async () => {
  setCsrfCookie()
  const server = stubSpaceServer(true)
  window.history.replaceState(null, "", "/spaces")
  holdControllers()
  const user = userEvent.setup()

  const first = render(<App />)
  act(() => controllers[0]?.onRuntime(ONE_RUNTIME_FIXTURE))
  const firstForm = within(screen.getByRole("form", { name: "Create a Space" }))
  await user.type(firstForm.getByLabelText("Name"), "Staging")
  await user.click(firstForm.getByRole("button", { name: "Create" }))
  expect(await firstForm.findByText(/may already exist.*same name/)).toBeInTheDocument()
  first.unmount()

  render(<App />)
  act(() => controllers[1]?.onRuntime(ONE_RUNTIME_FIXTURE))
  await user.click(await screen.findByRole("button", { name: "Start new creation" }))
  expect(screen.queryByText(/Unresolved request for/)).not.toBeInTheDocument()
  const reloadedForm = within(screen.getByRole("form", { name: "Create a Space" }))
  await user.type(reloadedForm.getByLabelText("Name"), "Staging")
  await user.click(reloadedForm.getByRole("button", { name: "Create" }))
  expect(await reloadedForm.findByText("Space created by the Runtime.")).toBeInTheDocument()

  expect(server.observedIds).toHaveLength(2)
  expect(server.observedIds[1]).not.toBe(server.observedIds[0])
  expect(server.committed.size).toBe(2)
})

// A pending creation belongs only to its Runtime and active operator session.
test("a different Runtime or a ended session never inherits an unresolved Space creation", async () => {
  setCsrfCookie()
  const server = stubSpaceServer(true)
  window.history.replaceState(null, "", "/spaces")
  holdControllers()
  const user = userEvent.setup()

  const first = render(<App />)
  act(() => controllers[0]?.onRuntime(ONE_RUNTIME_FIXTURE))
  const firstForm = within(screen.getByRole("form", { name: "Create a Space" }))
  await user.type(firstForm.getByLabelText("Name"), "Staging")
  await user.click(firstForm.getByRole("button", { name: "Create" }))
  expect(await firstForm.findByText(/may already exist.*same name/)).toBeInTheDocument()
  first.unmount()

  // Another Runtime at the same origin must not show or reuse the first Runtime's request ID.
  const otherRuntime = {
    ...ONE_RUNTIME_FIXTURE,
    endpoint: { ...ONE_RUNTIME_FIXTURE.endpoint, id: "bb".repeat(32) },
  }
  const second = render(<App />)
  act(() => controllers[1]?.onRuntime(otherRuntime))
  expect(screen.queryByText(/Unresolved request for/)).not.toBeInTheDocument()
  second.unmount()

  // Signing out clears stored pending creation state from tab storage.
  const third = render(<App />)
  act(() => controllers[2]?.onRuntime(ONE_RUNTIME_FIXTURE))
  expect(await screen.findByText(/Unresolved request for “Staging”/)).toBeInTheDocument()
  act(() => controllers[2]?.onSessionExpired())
  expect(window.sessionStorage.length).toBe(0)
  third.unmount()

  expect(server.observedIds).toHaveLength(1)
})

// A fresh creation refused before dispatch (HTTP 422/403) must not persist or display an unresolved
// request, whereas an HTTP 403 on a retry of a lost attempt must keep the unresolved request intact.
test.each([422, 403])(
  "a fresh HTTP %i leaves no unresolved Space creation while a 403 retry keeps one",
  async (freshStatus) => {
    setCsrfCookie()
    const observedIds: string[] = []
    const answers: Array<"drop" | number> = [freshStatus, "drop", 403]
    vi.spyOn(globalThis, "fetch").mockImplementation(async (input, init): Promise<Response> => {
      const command = (await new Request(input, init).json()) as { request_id: string }
      observedIds.push(command.request_id)
      const next = answers.shift()
      if (next === "drop") throw new TypeError("Failed to fetch")
      return new Response(null, { status: next ?? 500 })
    })
    window.history.replaceState(null, "", "/spaces")
    holdControllers()
    const user = userEvent.setup()

    render(<App />)
    act(() => controllers[0]?.onRuntime(ONE_RUNTIME_FIXTURE))
    const form = within(screen.getByRole("form", { name: "Create a Space" }))
    await user.type(form.getByLabelText("Name"), "Staging")
    await user.click(form.getByRole("button", { name: "Create" }))
    expect(await form.findByText("The Runtime rejected the request.")).toBeInTheDocument()
    expect(screen.queryByText(/Unresolved request for/)).not.toBeInTheDocument()
    expect(window.sessionStorage.length).toBe(0)

    await user.click(form.getByRole("button", { name: "Create" }))
    expect(await form.findByText(/may already exist.*same name/)).toBeInTheDocument()
    expect(screen.getByText(/Unresolved request for “Staging”/)).toBeInTheDocument()

    await user.click(screen.getByRole("button", { name: "Retry pending creation" }))
    expect(await form.findByText("The Runtime rejected the request.")).toBeInTheDocument()
    expect(screen.getByText(/Unresolved request for “Staging”/)).toBeInTheDocument()
    expect(window.sessionStorage.length).toBe(1)
    expect(observedIds).toHaveLength(3)
    expect(observedIds[1]).not.toBe(observedIds[0])
    expect(observedIds[2]).toBe(observedIds[1])
  },
)
