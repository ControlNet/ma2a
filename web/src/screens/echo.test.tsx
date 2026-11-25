import "@testing-library/jest-dom/vitest"

import { fireEvent, render, screen, within } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { expect, test, vi } from "vitest"

import { ONE_RUNTIME_FIXTURE } from "../test/fixtures"
import { runtimeActions } from "./actions.fixture"
import { EchoScreen } from "./echo"

test("reports the responder-measured duration against the documented deadline", async () => {
  const user = userEvent.setup()
  const actions = runtimeActions()
  render(<EchoScreen actions={actions} runtime={ONE_RUNTIME_FIXTURE} />)
  const form = within(screen.getByRole("form", { name: "Send an Echo" }))

  await user.type(form.getByLabelText("Target Endpoint ID"), "11".repeat(32))
  await user.click(form.getByRole("button", { name: "Send Echo" }))

  const reply = await screen.findByRole("status")
  expect(reply).toHaveTextContent("34 ms")
  expect(within(reply).getByRole("list", { name: "Echo deadline" })).toHaveTextContent(
    "34 / 10000 ms",
  )
})

test("counts payload bytes rather than characters against the 4096 byte bound", async () => {
  const user = userEvent.setup()
  render(<EchoScreen actions={runtimeActions()} runtime={ONE_RUNTIME_FIXTURE} />)

  const payload = screen.getByLabelText("Payload")
  await user.clear(payload)
  await user.type(payload, "éé")

  expect(screen.getByRole("list", { name: "Echo bounds" })).toHaveTextContent("4 / 4096")
})

test("sends one Echo at a time, so a late reply never overwrites a newer one", async () => {
  const user = userEvent.setup()
  const actions = runtimeActions()
  let fail: (error: Error) => void = () => undefined
  vi.mocked(actions.echo).mockImplementationOnce(
    () => new Promise((_resolve, reject) => (fail = reject)),
  )
  render(<EchoScreen actions={actions} runtime={ONE_RUNTIME_FIXTURE} />)
  const form = within(screen.getByRole("form", { name: "Send an Echo" }))
  await user.type(form.getByLabelText("Target Endpoint ID"), "11".repeat(32))

  await user.click(form.getByRole("button", { name: "Send Echo" }))
  fireEvent.submit(screen.getByRole("form", { name: "Send an Echo" }))

  expect(actions.echo).toHaveBeenCalledTimes(1)
  expect(form.getByRole("button", { name: "Send Echo" })).toBeDisabled()
  fail(new Error("timed out"))
  expect(await screen.findByText("Echo failed or was not authorized.")).toBeVisible()
  expect(form.getByRole("button", { name: "Send Echo" })).toBeEnabled()
})
