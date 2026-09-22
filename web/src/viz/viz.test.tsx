import "@testing-library/jest-dom/vitest"

import { render, screen } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { expect, test, vi } from "vitest"

import { Donut } from "./donut"
import { LaneMap } from "./lane-map"
import { MeterList } from "./meter-list"
import { Ring } from "./ring"
import { StackBar } from "./stack-bar"
import { StateMachine } from "./state-machine"
import { TrustChain } from "./trust-chain"

const LANES = [
  { spaceId: "9f2c41d8", name: "Operations", memberCount: 1 },
  { spaceId: "3b7e05c1", name: "Laboratory", memberCount: 3 },
] as const

const PEERS = [
  {
    id: "4444444444444444",
    shortId: "4444\u20264444",
    state: "connected",
    tone: "direct",
    badge: "direct 34 ms",
  },
  {
    id: "a1b2c3d4e5f60718",
    shortId: "a1b2\u20260718",
    state: "no observation",
    tone: "none",
    badge: "never seen",
  },
] as const

test("a ring states its own value instead of relying on the arc", () => {
  render(<Ring fraction={0.47} label="next sync" value="00:47" />)

  expect(screen.getByRole("img", { name: "next sync: 00:47" })).toBeInTheDocument()
})

test("a donut names every segment it draws", () => {
  render(
    <Donut
      label="echo"
      segments={[
        { tone: "direct", value: 1, label: "ok" },
        { tone: "failed", value: 1, label: "failed" },
      ]}
      value="2"
    />,
  )

  expect(screen.getByRole("img", { name: /1 ok, 1 failed/ })).toBeInTheDocument()
})

test("meters spell out the bound so the bar is never the only reading", () => {
  render(
    <MeterList
      label="Bounded capacity"
      meters={[{ label: "Connections", fraction: 1 / 128, value: "1 / 128", tone: "accent" }]}
    />,
  )

  const list = screen.getByRole("list", { name: "Bounded capacity" })
  expect(list).toHaveTextContent("Connections")
  expect(list).toHaveTextContent("1 / 128")
})

test("a stacked bar keeps a text legend for every part, including empty ones", () => {
  render(
    <StackBar
      caption="Observed paths"
      parts={[
        { label: "direct", value: 1, tone: "direct" },
        { label: "relay", value: 0, tone: "relay" },
        { label: "none", value: 1, tone: "none" },
      ]}
    />,
  )

  expect(screen.getByText(/relay 0/)).toBeInTheDocument()
})

test("exactly one reachability state is marked current", () => {
  render(
    <StateMachine
      current="AwaitingIrohHome"
      states={[
        { name: "NoActiveSpaces", gloss: "a", tone: "none" },
        { name: "DegradedNoCommonHome", gloss: "b", tone: "failed" },
        { name: "AwaitingIrohHome", gloss: "c", tone: "relay" },
        { name: "IrohHomeConnected", gloss: "d", tone: "direct" },
      ]}
    />,
  )

  const current = screen.getAllByRole("listitem").filter((item) => item.ariaCurrent === "true")
  expect(current).toHaveLength(1)
  expect(current[0]).toHaveTextContent("AwaitingIrohHome")
})

test("signed lanes and observed peers are separate, labelled bands", () => {
  render(<LaneMap lanes={LANES} peers={PEERS} />)

  expect(screen.getByRole("heading", { name: "Spaces" })).toBeInTheDocument()
  expect(screen.getByRole("heading", { name: "Observed Peers" })).toBeInTheDocument()
  expect(screen.getByText("3 members")).toBeInTheDocument()
})

test("a peer that was never observed keeps its place in the observed band", () => {
  render(<LaneMap lanes={LANES} peers={PEERS} />)

  expect(screen.getByText("never seen")).toBeInTheDocument()
})

test("selecting a peer reports the full identifier, not the abbreviation", async () => {
  const user = userEvent.setup()
  const onSelect = vi.fn()
  render(<LaneMap lanes={LANES} onSelect={onSelect} peers={PEERS} selected="4444444444444444" />)

  await user.click(screen.getByRole("button", { name: /never seen/ }))

  expect(onSelect).toHaveBeenCalledWith("a1b2c3d4e5f60718")
  expect(screen.getByRole("button", { name: /direct 34 ms/ })).toHaveAttribute(
    "aria-pressed",
    "true",
  )
})

test("zero Spaces explains the protocol isolation instead of drawing nothing", () => {
  render(<LaneMap lanes={[]} peers={[]} />)

  expect(screen.getByText(/No Space yet/)).toBeInTheDocument()
  expect(screen.getByText(/ma2a\/enrollment\/1/)).toBeInTheDocument()
})

test("the trust chain separates transient handling from retention", () => {
  render(
    <TrustChain
      boundaries={[
        { name: "Browser", guard: "session cookie", presence: ["transient", "never"] },
        { name: "Protected storage", guard: "0600 key files", presence: ["never", "retained"] },
      ]}
      secrets={["password", "private keys"]}
    />,
  )

  expect(screen.getByText("handled transiently, not retained")).toBeInTheDocument()
  expect(screen.getByText("retained here")).toBeInTheDocument()
  expect(screen.getAllByText("never present here")).toHaveLength(2)
})
