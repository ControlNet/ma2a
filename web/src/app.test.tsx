import "@testing-library/jest-dom/vitest"

import { render, screen } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import axe from "axe-core"

import { App } from "./app"
import { EMPTY_RUNTIME_FIXTURE, MANY_RUNTIME_FIXTURE, ONE_RUNTIME_FIXTURE } from "./test/fixtures"

Object.defineProperty(HTMLElement.prototype, "scrollIntoView", {
  configurable: true,
  value: () => undefined,
})

const ROUTES = [
  "/login",
  "/setup",
  "/",
  "/endpoint",
  "/spaces",
  "/relays",
  "/echo",
  "/settings",
] as const

describe.each(ROUTES)("route %s", (path) => {
  test("has no serious or critical axe findings", async () => {
    const given = render(<App initialPath={path} runtime={MANY_RUNTIME_FIXTURE} />)

    const when = await axe.run(given.container)

    expect(
      when.violations.filter(
        (violation) => violation.impact === "serious" || violation.impact === "critical",
      ),
    ).toEqual([])
  })
})

test("moves keyboard focus to main content through the skip link", async () => {
  const user = userEvent.setup()
  render(<App initialPath="/" runtime={MANY_RUNTIME_FIXTURE} />)

  await user.tab()
  await user.keyboard("{Enter}")

  expect(screen.getByRole("main")).toHaveFocus()
})

test.each([
  [EMPTY_RUNTIME_FIXTURE, "No Spaces yet"],
  [ONE_RUNTIME_FIXTURE, "Operations"],
  [MANY_RUNTIME_FIXTURE, "Laboratory"],
] as const)("renders zero, one, and many Space states", (runtime, expectedText) => {
  render(<App initialPath="/spaces" runtime={runtime} />)

  expect(screen.getByText(expectedText)).toBeVisible()
})

test.each([
  [EMPTY_RUNTIME_FIXTURE, "No relay candidates"],
  [ONE_RUNTIME_FIXTURE, "relay.test.invalid"],
  [MANY_RUNTIME_FIXTURE, "space-relay.test.invalid"],
] as const)("renders zero, one, and many Relay states", (runtime, expectedText) => {
  render(<App initialPath="/relays" runtime={runtime} />)

  expect(screen.getByText(expectedText, { exact: false })).toBeVisible()
})

test("keeps password setup on trusted CLI only", () => {
  render(<App initialPath="/setup" runtime={EMPTY_RUNTIME_FIXTURE} />)

  expect(screen.getByText("ma2a ui password set")).toBeVisible()
  expect(screen.queryByLabelText("New password")).not.toBeInTheDocument()
})
