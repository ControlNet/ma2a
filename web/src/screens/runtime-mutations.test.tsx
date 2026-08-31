import "@testing-library/jest-dom/vitest"

import { render, screen, within } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { expect, test, vi } from "vitest"

import type { RuntimeActions } from "../runtime-actions"
import { RelaysScreen } from "./relays"
import { SpacesScreen } from "./spaces"

function runtimeActions(): RuntimeActions {
  const configurePrivateRelay = vi.fn<RuntimeActions["configurePrivateRelay"]>(async () => ({
    configured: true,
    mode: "external_termination",
    host: "127.0.0.1",
    port: 443,
    online: true,
  }))
  return {
    createSpace: vi.fn(async () => undefined),
    createInvitation: vi.fn(async () => undefined),
    revokeEndpoint: vi.fn(async () => undefined),
    triggerSync: vi.fn(async () => undefined),
    configurePrivateRelay,
    configurePublicRelay: vi.fn(async () => ({ configured: true, url: null, online: false })),
    echo: vi.fn(async () => ({ target_endpoint_id: "11".repeat(32), payload: "ok" })),
    revokeSessions: vi.fn(async () => undefined),
  }
}

test("creates an invitation ticket through the Runtime without browser secret material", async () => {
  const user = userEvent.setup()
  const actions = runtimeActions()
  render(<SpacesScreen actions={actions} runtime={undefined} />)
  const form = screen.getByRole("heading", { name: "Create Invitation" }).closest("form")
  expect(form).not.toBeNull()
  if (form === null) return
  const invitation = within(form)

  await user.type(invitation.getByLabelText("Space ID"), "ab".repeat(32))
  await user.type(invitation.getByLabelText("Lifetime in milliseconds"), "300000")
  await user.type(invitation.getByLabelText("Local output path"), "/tmp/operations.invite")
  expect(form).toBeValid()
  await user.click(invitation.getByRole("button", { name: "Create ticket" }))

  expect(actions.createInvitation).toHaveBeenCalledWith(
    "ab".repeat(32),
    300_000,
    "/tmp/operations.invite",
  )
})

test("configures external Private Relay termination with parsed Space IDs and null TLS paths", async () => {
  const user = userEvent.setup()
  const actions = runtimeActions()
  render(<RelaysScreen actions={actions} runtime={undefined} />)
  const form = screen.getByRole("heading", { name: "Private Provider" }).closest("form")
  expect(form).not.toBeNull()
  if (form === null) return
  const privateRelay = within(form)

  await user.selectOptions(privateRelay.getByLabelText("TLS termination"), "external_termination")
  await user.type(privateRelay.getByLabelText("Listen address"), "127.0.0.1:443")
  await user.type(privateRelay.getByLabelText("Public HTTPS URL"), "https://relay.example")
  await user.type(
    privateRelay.getByLabelText("Served Space IDs"),
    `${"ab".repeat(32)},\n${"cd".repeat(32)}`,
  )
  expect(form).toBeValid()
  await user.click(privateRelay.getByRole("button", { name: "Configure Private" }))

  expect(actions.configurePrivateRelay).toHaveBeenCalledWith({
    mode: "external_termination",
    listen: "127.0.0.1:443",
    publicUrl: "https://relay.example",
    servedSpaceIds: ["ab".repeat(32), "cd".repeat(32)],
    certificatePath: null,
    privateKeyPath: null,
  })
})
