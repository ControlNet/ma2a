import "@testing-library/jest-dom/vitest"

import { render, screen } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { expect, test, vi } from "vitest"

import { ONE_RUNTIME_FIXTURE } from "../test/fixtures"
import { runtimeActions } from "./actions.fixture"
import { SettingsInspector, SettingsScreen } from "./settings"

test("revoke all forces local sign-out without calling authenticated logout", async () => {
  const user = userEvent.setup()
  const actions = runtimeActions()
  const onLogout = vi.fn(() => Promise.resolve())
  const onSessionsRevoked = vi.fn()
  render(
    <SettingsInspector
      actions={actions}
      onLogout={onLogout}
      onSessionsRevoked={onSessionsRevoked}
      runtime={ONE_RUNTIME_FIXTURE}
    />,
  )

  await user.click(screen.getByRole("button", { name: "Revoke all sessions" }))

  expect(actions.revokeSessions).toHaveBeenCalledOnce()
  expect(onSessionsRevoked).toHaveBeenCalledOnce()
  expect(onLogout).not.toHaveBeenCalled()
})

test("the trust boundary is about retention, and says so about the passphrase", () => {
  render(<SettingsScreen runtime={ONE_RUNTIME_FIXTURE} />)

  const browser = screen.getByRole("columnheader", { name: /Browser/ })
  expect(browser).toBeInTheDocument()
  expect(screen.getAllByText("handled transiently, not retained").length).toBeGreaterThan(0)
  expect(screen.getAllByText("retained here").length).toBeGreaterThan(0)
  expect(screen.getByText("ma2a ui password reset")).toBeInTheDocument()
  expect(screen.getByText(/Handling is not retention/)).toBeInTheDocument()
})
