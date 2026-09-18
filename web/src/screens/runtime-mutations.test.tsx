import "@testing-library/jest-dom/vitest"

import { render, screen, within } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { expect, test } from "vitest"

import { ONE_RUNTIME_FIXTURE } from "../test/fixtures"
import { runtimeActions } from "./actions.fixture"
import { RelaysInspector } from "./relays"
import { SpacesInspector } from "./spaces"

test("creates an invitation ticket through the Runtime without browser secret material", async () => {
  const user = userEvent.setup()
  const actions = runtimeActions()
  render(<SpacesInspector actions={actions} runtime={ONE_RUNTIME_FIXTURE} />)
  const form = within(screen.getByRole("form", { name: "Create an invitation ticket" }))

  await user.type(form.getByLabelText("Owner-only output path"), "/tmp/operations.invite")
  await user.click(form.getByRole("button", { name: "Write ticket" }))

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
  const form = within(screen.getByRole("form", { name: "Create an invitation ticket" }))

  await user.type(form.getByLabelText("Owner-only output path"), "/tmp/operations.invite")
  await user.click(form.getByRole("button", { name: "Write ticket" }))

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
