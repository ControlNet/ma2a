import "@testing-library/jest-dom/vitest"

import { render, screen, within } from "@testing-library/react"
import { expect, test, vi } from "vitest"

import { EMPTY_RUNTIME_FIXTURE, MANY_RUNTIME_FIXTURE } from "../test/fixtures"
import { SERVICES_FIXTURE } from "../test/services-fixture"
import { ServicesScreen } from "./services"
import { byteCount, duration } from "./services-format"

test("lists each exposure under its Space name with the scope it serves", () => {
  render(
    <ServicesScreen
      runtime={MANY_RUNTIME_FIXTURE}
      services={{ status: "ready", data: SERVICES_FIXTURE }}
    />,
  )

  const exposures = screen.getByRole("table", { name: "Services this Runtime serves, by Space" })
  const rows = within(exposures).getAllByRole("row").slice(1)
  expect(rows.map((row) => row.textContent)).toEqual([
    "LaboratoryFilesdocs:/ → /srv/docs (read-only)",
    "LaboratoryProxy127.0.0.1 port 8000-8080",
    "ababab…ababExecone-shot programs",
  ])
})

test("reports active and recent operations with their bounded audit", () => {
  render(
    <ServicesScreen
      runtime={MANY_RUNTIME_FIXTURE}
      services={{ status: "ready", data: SERVICES_FIXTURE }}
    />,
  )

  const active = screen.getByRole("table", { name: "Operations running now" })
  expect(active).toHaveTextContent("shell shell")
  expect(active).toHaveTextContent("1 m 5 s")
  expect(screen.getByText("Recent operations · latest 2 of 300")).toBeVisible()
  const recent = screen.getByRole("table", { name: "Recently finished operations, newest first" })
  expect(recent).toHaveTextContent("inbound transfer download")
  expect(recent).toHaveTextContent("3.0 MiB")
  expect(recent).toHaveTextContent("rejected: unauthorized")
})

test("says nothing is served rather than drawing an empty table", () => {
  render(
    <ServicesScreen
      runtime={EMPTY_RUNTIME_FIXTURE}
      services={{
        status: "ready",
        data: {
          exposures: [],
          operations: { active_count: 0, active: [], recent_count: 0, recent: [] },
          processes: { processes: [], next: null },
          agents: { agents: [], more_agents: false, tasks: [], more_tasks: false },
          hosted: { services: [], more: false },
        },
      }}
    />,
  )

  expect(screen.getByText("Nothing is served")).toBeVisible()
  expect(screen.getByText("Nothing running")).toBeVisible()
  expect(screen.getByText("No Process")).toBeVisible()
  expect(screen.getByText("No Agent")).toBeVisible()
  expect(screen.getByText("No task")).toBeVisible()
  expect(screen.getByText("No hosted service")).toBeVisible()
  expect(screen.queryByRole("table")).toBeNull()
})

test("waits for, and reports failure of, the services read", () => {
  const pending = render(
    <ServicesScreen runtime={MANY_RUNTIME_FIXTURE} services={{ status: "pending" }} />,
  )
  expect(screen.getByRole("status")).toHaveTextContent("Waiting for an authoritative snapshot")
  pending.unmount()

  render(<ServicesScreen runtime={MANY_RUNTIME_FIXTURE} services={{ status: "failed" }} />)
  expect(screen.getByText("Services could not be loaded")).toBeVisible()
})

test("formats byte counts and durations exactly at their unit boundaries", () => {
  expect([byteCount(0), byteCount(1023), byteCount(1024), byteCount(5 * 1024 ** 3)]).toEqual([
    "0 B",
    "1023 B",
    "1.0 KiB",
    "5.0 GiB",
  ])
  expect([duration(999), duration(1_000), duration(59_999), duration(3_600_000)]).toEqual([
    "999 ms",
    "1.0 s",
    "60.0 s",
    "1 h 0 m",
  ])
})

test("lists hosted Processes by creator with their state and outcome only", () => {
  render(
    <ServicesScreen
      runtime={MANY_RUNTIME_FIXTURE}
      services={{ status: "ready", data: SERVICES_FIXTURE }}
    />,
  )

  expect(screen.getByText("Hosted Processes · 2")).toBeVisible()
  const hosted = screen.getByRole("table", { name: "Processes this Runtime hosts" })
  const rows = within(hosted).getAllByRole("row").slice(1)
  expect(rows.map((row) => row.textContent)).toEqual([
    "ticker444444…4444running",
    "0e0e0e…0e0e555555…5555terminated (archived)unauthorized, signal 15",
  ])
})

test("lists Agents and their tasks without programs, arguments, or messages", () => {
  render(
    <ServicesScreen
      runtime={MANY_RUNTIME_FIXTURE}
      services={{ status: "ready", data: SERVICES_FIXTURE }}
    />,
  )

  expect(screen.getByText("Agents · 2")).toBeVisible()
  const agents = screen.getByRole("table", { name: "Agents this Runtime hosts" })
  expect(
    within(agents)
      .getAllByRole("row")
      .slice(1)
      .map((row) => row.textContent),
  ).toEqual([
    "helperanswers questionsready (generation 2)files, agent1 of 4",
    "idlestoppednone0 of 1",
  ])
  expect(screen.getByText("Agent tasks · 2 (first page)")).toBeVisible()
  const tasks = screen.getByRole("table", { name: "Agent tasks, by creator" })
  expect(
    within(tasks)
      .getAllByRole("row")
      .slice(1)
      .map((row) => row.textContent),
  ).toEqual([
    "0b0b0b…0b0bhelper444444…4444 (from lead)running",
    "0c0c0c…0c0chelper555555…5555cancelled (unauthorized)",
  ])
})

test("lists hosted services with their versions, state, and call counts only", () => {
  render(
    <ServicesScreen
      runtime={MANY_RUNTIME_FIXTURE}
      services={{ status: "ready", data: SERVICES_FIXTURE }}
    />,
  )

  expect(screen.getByText("Hosted services · 2")).toBeVisible()
  const hosted = screen.getByRole("table", { name: "Services this Runtime hosts" })
  expect(
    within(hosted)
      .getAllByRole("row")
      .slice(1)
      .map((row) => row.textContent),
  ).toEqual([
    "exampleDeterministic example serviceready1, 21 of 4v1: 3, v2: 5",
    "retiredstopped10 of 1none yet",
  ])
})

test("renders active and recent operations sharing direction and request_id without duplicate keys", () => {
  const errorSpy = vi.spyOn(console, "error").mockImplementation(() => undefined)
  try {
    const sharedId = "01".repeat(16)
    render(
      <ServicesScreen
        runtime={MANY_RUNTIME_FIXTURE}
        services={{
          status: "ready",
          data: {
            ...SERVICES_FIXTURE,
            operations: {
              active_count: 2,
              active: [
                {
                  request_id: sharedId,
                  peer_endpoint_id: "44".repeat(32),
                  service: "shell",
                  operation: "shell",
                  direction: "inbound",
                  elapsed_ms: 1_000,
                },
                {
                  request_id: sharedId,
                  peer_endpoint_id: "55".repeat(32),
                  service: "exec",
                  operation: "exec",
                  direction: "inbound",
                  elapsed_ms: 2_000,
                },
              ],
              recent_count: 2,
              recent: [
                {
                  request_id: sharedId,
                  peer_endpoint_id: "44".repeat(32),
                  service: "files",
                  operation: "read",
                  direction: "inbound",
                  result: "completed",
                  error: null,
                  bytes_in: 10,
                  bytes_out: 20,
                  duration_ms: 5,
                },
                {
                  request_id: sharedId,
                  peer_endpoint_id: "44".repeat(32),
                  service: "files",
                  operation: "read",
                  direction: "inbound",
                  result: "rejected",
                  error: "unauthorized",
                  bytes_in: 0,
                  bytes_out: 0,
                  duration_ms: 1,
                },
              ],
            },
          },
        }}
      />,
    )

    const active = screen.getByRole("table", { name: "Operations running now" })
    expect(within(active).getAllByRole("row").slice(1)).toHaveLength(2)
    const recent = screen.getByRole("table", { name: "Recently finished operations, newest first" })
    expect(within(recent).getAllByRole("row").slice(1)).toHaveLength(2)
    expect(errorSpy).not.toHaveBeenCalled()
  } finally {
    errorSpy.mockRestore()
  }
})
