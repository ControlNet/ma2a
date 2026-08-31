import "@testing-library/jest-dom/vitest"

import { render, screen } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { expect, test, vi } from "vitest"

import type { RuntimeActions } from "../runtime-actions"
import { SettingsScreen } from "./settings"

test("revoke all forces local sign-out without calling authenticated logout", async () => {
  const user = userEvent.setup()
  const revokeSessions = vi.fn(() => Promise.resolve())
  const onLogout = vi.fn(() => Promise.resolve())
  const onSessionsRevoked = vi.fn()
  const actions: RuntimeActions = {
    createSpace: vi.fn(() => Promise.resolve()),
    createInvitation: vi.fn(() => Promise.resolve()),
    revokeEndpoint: vi.fn(() => Promise.resolve()),
    triggerSync: vi.fn(() => Promise.resolve()),
    configurePrivateRelay: vi.fn(() => Promise.reject(new TypeError("unused"))),
    configurePublicRelay: vi.fn(() => Promise.reject(new TypeError("unused"))),
    echo: vi.fn(() => Promise.reject(new TypeError("unused"))),
    revokeSessions,
  }
  render(
    <SettingsScreen actions={actions} onLogout={onLogout} onSessionsRevoked={onSessionsRevoked} />,
  )

  await user.click(screen.getByRole("button", { name: "Revoke all" }))

  expect(revokeSessions).toHaveBeenCalledOnce()
  expect(onSessionsRevoked).toHaveBeenCalledOnce()
  expect(onLogout).not.toHaveBeenCalled()
})
