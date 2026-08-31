import "@testing-library/jest-dom/vitest"

import { render, screen } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { expect, test, vi } from "vitest"

import { RuntimeLoadError } from "./runtime-status"

test("initial snapshot failure renders an actionable blocking error", async () => {
  const user = userEvent.setup()
  const onRetry = vi.fn()
  render(<RuntimeLoadError onRetry={onRetry} />)

  expect(screen.getByRole("alert")).toHaveTextContent("Runtime snapshot is unavailable")
  await user.click(screen.getByRole("button", { name: "Retry" }))

  expect(onRetry).toHaveBeenCalledOnce()
})
