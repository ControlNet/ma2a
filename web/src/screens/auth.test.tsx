import "@testing-library/jest-dom/vitest"

import { fireEvent, render, screen } from "@testing-library/react"
import { expect, test } from "vitest"

import { LoginScreen } from "./auth"

test("a sign-in still in flight is not sent again", () => {
  let attempts = 0
  const onLogin = (): Promise<void> => {
    attempts += 1
    return new Promise<void>(() => undefined)
  }
  render(<LoginScreen onLogin={onLogin} />)
  fireEvent.change(screen.getByLabelText("Password"), {
    target: { value: "browser-login-password-9!" },
  })
  const form = screen.getByLabelText("Password").closest("form")
  if (form === null) throw new Error("the password sits in a form")

  fireEvent.submit(form)
  fireEvent.submit(form)

  expect(attempts).toBe(1)
})
