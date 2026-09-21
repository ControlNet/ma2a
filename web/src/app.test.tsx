import "@testing-library/jest-dom/vitest"

import { render, screen, within } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import axe from "axe-core"
import { describe, expect, test } from "vitest"

import { App } from "./app"
import { EMPTY_RUNTIME_FIXTURE, MANY_RUNTIME_FIXTURE, ONE_RUNTIME_FIXTURE } from "./test/fixtures"

Object.defineProperty(HTMLElement.prototype, "scrollIntoView", {
  configurable: true,
  value: () => undefined,
  writable: true,
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
  expect(screen.getByRole("link", { name: "Skip to content" })).toHaveFocus()
  await user.keyboard("{Enter}")

  expect(screen.getByRole("main")).toHaveFocus()
})

test("keeps the browser sequential focus origin at the document start", () => {
  let scrollIntoViewCalls = 0
  HTMLElement.prototype.scrollIntoView = () => {
    scrollIntoViewCalls += 1
  }

  render(<App initialPath="/settings" runtime={MANY_RUNTIME_FIXTURE} />)

  expect(scrollIntoViewCalls).toBe(0)
})

test("makes the named main landmark the keyboard-scrollable content region", () => {
  render(<App initialPath="/settings" runtime={MANY_RUNTIME_FIXTURE} />)

  const main = screen.getByRole("main", { name: "Runtime content" })
  expect(main).toHaveAttribute("tabindex", "0")
})

test("every destination stays reachable at every width, with the current one marked", () => {
  render(<App initialPath="/settings" runtime={MANY_RUNTIME_FIXTURE} />)

  const navigation = screen.getByRole("navigation", { name: "Console sections" })
  const links = within(navigation).getAllByRole("link")

  expect(links).toHaveLength(6)
  expect(links.filter((link) => link.getAttribute("aria-current") === "page")).toHaveLength(1)
  expect(within(navigation).getByRole("link", { name: "Settings" })).toHaveAttribute(
    "aria-current",
    "page",
  )
})

test.each([
  [EMPTY_RUNTIME_FIXTURE, "No Space yet"],
  [ONE_RUNTIME_FIXTURE, "Signed members"],
  [MANY_RUNTIME_FIXTURE, "generation 11"],
] as const)("renders zero, one, and many Space states", (runtime, expectedText) => {
  render(<App initialPath="/spaces" runtime={runtime} />)

  expect(screen.getAllByText(expectedText, { exact: false })[0]).toBeVisible()
})

test("a Space shows its signed member set, not just a count", () => {
  render(<App initialPath="/spaces" runtime={MANY_RUNTIME_FIXTURE} />)

  const shared = screen.getAllByText("field-station-2")
  expect(shared).toHaveLength(2)
  expect(screen.getByText("lab-archive")).toBeVisible()
  expect(screen.getByText(/1 revoked, carried forward/)).toBeVisible()
})

test("the raw per-member relay-provider bit is not shown as effective permission", () => {
  render(<App initialPath="/spaces" runtime={MANY_RUNTIME_FIXTURE} />)

  expect(screen.queryByText("relay-provider")).not.toBeInTheDocument()
})

test("no Space claims a global control-sync status", () => {
  render(<App initialPath="/spaces" runtime={MANY_RUNTIME_FIXTURE} />)

  expect(screen.queryByText(/control sync/i)).not.toBeInTheDocument()
})

test("a public fallback is listed without any Endpoint identity", () => {
  render(<App initialPath="/relays" runtime={MANY_RUNTIME_FIXTURE} />)

  expect(screen.getByText("https://public.example")).toBeVisible()
  expect(screen.getByText("external transport, no Endpoint identity")).toBeVisible()
})

test("an incompatible advertisement stays visible but is not counted as supplied", () => {
  render(<App initialPath="/relays" runtime={MANY_RUNTIME_FIXTURE} />)

  expect(screen.getByText("https://relay.ops.internal")).toBeVisible()
  expect(screen.getAllByText("NO").length).toBeGreaterThan(0)
  expect(screen.getByText("2 supplied")).toBeVisible()
})

test("the local provider role is not filed under Iroh observations", () => {
  render(<App initialPath="/relays" runtime={MANY_RUNTIME_FIXTURE} />)

  const provider = screen.getByRole("list", { name: "Local provider role" })
  expect(provider).toHaveTextContent("Private Relay Provider running here")
  expect(screen.getByRole("list", { name: "Iroh transport observation" })).not.toHaveTextContent(
    "Private Relay Provider",
  )
})

test("a running local Private Relay Provider is not reported as a connected home", () => {
  render(<App initialPath="/relays" runtime={MANY_RUNTIME_FIXTURE} />)

  const current = screen.getAllByRole("listitem").filter((item) => item.ariaCurrent === "true")
  expect(current[0]).toHaveTextContent("DegradedNoCommonHome")
  expect(screen.getByText("Private Relay Provider running here")).toBeVisible()
})

test("a signed member with no observation still appears under Peers", () => {
  render(<App initialPath="/endpoint" runtime={MANY_RUNTIME_FIXTURE} />)

  expect(screen.getByText("lab-archive")).toBeVisible()
  expect(screen.getAllByText("no transport observation").length).toBeGreaterThan(0)
})

test("relay coverage is drawn as a grid whose verdict follows the cells", () => {
  render(<App initialPath="/relays" runtime={MANY_RUNTIME_FIXTURE} />)

  expect(screen.getByRole("columnheader", { name: "Home-compatible" })).toBeInTheDocument()
  expect(screen.getAllByText("covered").length).toBeGreaterThan(0)
  expect(screen.getAllByText("not covered").length).toBeGreaterThan(0)
  expect(screen.getAllByText("NO").length).toBeGreaterThan(0)
})

test.each([
  [EMPTY_RUNTIME_FIXTURE, "No relay advertisement or fallback"],
  [ONE_RUNTIME_FIXTURE, "https://relay.ops.internal"],
  [MANY_RUNTIME_FIXTURE, "https://relay.lab.internal"],
] as const)("renders zero, one, and many Relay states", (runtime, expectedText) => {
  render(<App initialPath="/relays" runtime={runtime} />)

  expect(screen.getAllByText(expectedText, { exact: false })[0]).toBeVisible()
})

test.each([
  [EMPTY_RUNTIME_FIXTURE, "NoActiveSpaces"],
  [ONE_RUNTIME_FIXTURE, "AwaitingIrohHome"],
  [MANY_RUNTIME_FIXTURE, "DegradedNoCommonHome"],
] as const)("names the reachability state the Runtime reports", (runtime, expected) => {
  render(<App initialPath="/relays" runtime={runtime} />)

  const current = screen.getAllByRole("listitem").filter((item) => item.ariaCurrent === "true")
  expect(current).toHaveLength(1)
  expect(current[0]).toHaveTextContent(expected)
})

test("keeps password setup on trusted CLI only", () => {
  render(<App initialPath="/setup" runtime={EMPTY_RUNTIME_FIXTURE} />)

  expect(screen.getByText("ma2a ui start")).toBeVisible()
  expect(screen.getByText("ma2a ui init")).toBeVisible()
  expect(screen.queryByLabelText("New password")).not.toBeInTheDocument()
})

test("uses the reset command for Settings password recovery", () => {
  render(<App initialPath="/settings" runtime={MANY_RUNTIME_FIXTURE} />)

  expect(screen.getByText("ma2a ui init")).toBeVisible()
  expect(screen.queryByText("ma2a ui password set")).not.toBeInTheDocument()
})
