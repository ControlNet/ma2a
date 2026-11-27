import "@testing-library/jest-dom/vitest"

import { render, screen, within } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { afterEach, expect, test, vi } from "vitest"
import type { RequestId } from "../api/generated"
import { createRuntimeMutationClient, RuntimeMutationRejectedError } from "../api/mutations"
import { createRuntimeActions } from "../runtime-actions"
import { ONE_RUNTIME_FIXTURE } from "../test/fixtures"
import { runtimeActions } from "./actions.fixture"
import { RelaysInspector } from "./relay-config"
import { SpacesInspector } from "./spaces"

afterEach(() => {
  vi.restoreAllMocks()
})

test("creates an invitation ticket through the Runtime without browser secret material", async () => {
  const user = userEvent.setup()
  const actions = runtimeActions()
  render(<SpacesInspector actions={actions} runtime={ONE_RUNTIME_FIXTURE} />)
  const form = within(screen.getByRole("form", { name: "Invite to a Space" }))

  await user.type(form.getByLabelText("Owner-only ticket file"), "/tmp/operations.invite")
  await user.click(form.getByRole("button", { name: "Create invite" }))

  expect(actions.createInvitation).toHaveBeenCalledWith(
    "test-space-operations",
    300_000,
    "/tmp/operations.invite",
  )
})

test("the invitation form never renders the ticket secret it asks the Runtime to write", async () => {
  const user = userEvent.setup()
  const actions = runtimeActions()
  render(<SpacesInspector actions={actions} runtime={ONE_RUNTIME_FIXTURE} />)
  const form = within(screen.getByRole("form", { name: "Invite to a Space" }))

  await user.type(form.getByLabelText("Owner-only ticket file"), "/tmp/operations.invite")
  await user.click(form.getByRole("button", { name: "Create invite" }))

  expect(await screen.findByText(/secret never reaches this browser/)).toBeInTheDocument()
})

test("configures external Private Relay termination with parsed Space IDs and null TLS paths", async () => {
  const user = userEvent.setup()
  const actions = runtimeActions()
  render(<RelaysInspector actions={actions} />)
  const form = within(screen.getByRole("form", { name: "Configure the Private Relay" }))

  await user.click(form.getByRole("button", { name: "External" }))
  await user.type(form.getByLabelText("Listen address"), "127.0.0.1:443")
  await user.type(form.getByLabelText("Public HTTPS URL"), "https://relay.example")
  await user.type(
    form.getByLabelText("Served Space IDs"),
    `${"ab".repeat(32)},\n${"cd".repeat(32)}`,
  )
  await user.click(form.getByRole("button", { name: "Configure private" }))

  expect(actions.configurePrivateRelay).toHaveBeenCalledWith({
    mode: "external_termination",
    listen: "127.0.0.1:443",
    publicUrl: "https://relay.example",
    servedSpaceIds: ["ab".repeat(32), "cd".repeat(32)],
    certificatePath: null,
    privateKeyPath: null,
  })
})

test("external termination hides the TLS path fields instead of ignoring them silently", async () => {
  const user = userEvent.setup()
  render(<RelaysInspector actions={runtimeActions()} />)

  expect(screen.getByLabelText("TLS private key path")).toBeInTheDocument()
  await user.click(screen.getByRole("button", { name: "External" }))

  expect(screen.queryByLabelText("TLS private key path")).not.toBeInTheDocument()
  expect(screen.getByText(/plaintext backend on loopback only/)).toBeInTheDocument()
})

// A lost answer is not a rejection: the Space may exist, and the same name retries safely.
// A bare `unavailable` from the Runtime (as when the prior attempt is still pending) stays unsettled.
test.each([
  [new TypeError("Failed to fetch"), /may already exist.*same name/],
  [new RuntimeMutationRejectedError("unavailable"), /may already exist.*same name/],
  [new RuntimeMutationRejectedError("conflict"), /^The Runtime rejected the request\.$/],
])("a failed Space creation says whether the Runtime answered: %s", async (error, status) => {
  const user = userEvent.setup()
  const actions = { ...runtimeActions(), createSpace: async () => Promise.reject(error) }
  render(<SpacesInspector actions={actions} runtime={ONE_RUNTIME_FIXTURE} />)
  const form = within(screen.getByRole("form", { name: "Create a Space" }))

  await user.type(form.getByLabelText("Name"), "Operations")
  await user.click(form.getByRole("button", { name: "Create" }))

  expect(await form.findByText(status)).toBeInTheDocument()
})

// The relay may already be configured when its answer was lost.
test("a relay configuration whose answer was lost does not say the Runtime rejected it", async () => {
  const user = userEvent.setup()
  const actions = {
    ...runtimeActions(),
    configurePublicRelay: async () => Promise.reject(new TypeError("Failed to fetch")),
  }
  render(<RelaysInspector actions={actions} />)
  const form = within(screen.getByRole("form", { name: "Configure the public fallback" }))

  await user.type(form.getByLabelText("HTTPS relay URL"), "https://relay.example")
  await user.click(form.getByRole("button", { name: "Configure public" }))

  expect(await form.findByText(/^No answer came back/)).toBeInTheDocument()
})

// Invalid Space IDs fail client-side validation before fetch, so the form must not say no answer came back.
test("an invalid Space ID in RelaysInspector shows a validation error without calling fetch", async () => {
  const fetchSpy = vi.spyOn(globalThis, "fetch")
  const user = userEvent.setup()
  const actions = createRuntimeActions(
    createRuntimeMutationClient({ csrfToken: () => "cd".repeat(32) }),
    async () => undefined,
    () => undefined,
  )
  render(<RelaysInspector actions={actions} />)
  const form = within(screen.getByRole("form", { name: "Configure the Private Relay" }))

  await user.click(form.getByRole("button", { name: "External" }))
  await user.type(form.getByLabelText("Listen address"), "127.0.0.1:443")
  await user.type(form.getByLabelText("Public HTTPS URL"), "https://relay.example")
  await user.type(form.getByLabelText("Served Space IDs"), "not-a-hex-id")
  await user.click(form.getByRole("button", { name: "Configure private" }))

  expect(await form.findByText(/64 lowercase hexadecimal/)).toBeInTheDocument()
  expect(form.queryByText(/No answer came back/)).not.toBeInTheDocument()
  expect(fetchSpy).not.toHaveBeenCalled()
})

test("refuses creating a different Space name while an unresolved creation is pending", async () => {
  const user = userEvent.setup()
  const actions = runtimeActions()
  render(
    <SpacesInspector
      actions={actions}
      pendingSpaceCreation={{
        endpointId: "aa".repeat(32),
        name: "Alpha",
        requestId: "01".repeat(16) as RequestId,
      }}
      runtime={ONE_RUNTIME_FIXTURE}
    />,
  )
  const form = within(screen.getByRole("form", { name: "Create a Space" }))

  await user.type(form.getByLabelText("Name"), "Beta")
  await user.click(form.getByRole("button", { name: "Create" }))

  expect(actions.createSpace).not.toHaveBeenCalled()
  expect(form.getByRole("status")).toHaveTextContent(/Alpha/)
})
