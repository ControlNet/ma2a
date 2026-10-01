import type { ReactNode } from "react"

import type { HostedProcess } from "../api/services"
import { shortId } from "./derive"

/** How one Process ended, or why it never ran; empty while it runs. */
export function processOutcome(process: HostedProcess): string {
  const parts = [
    process.reason?.replaceAll("_", " "),
    process.exit_code === null ? undefined : `exit ${process.exit_code}`,
    process.signal === null ? undefined : `signal ${process.signal}`,
  ].filter((part) => part !== undefined)
  return parts.join(", ")
}

/** Hosted Processes by creator; arguments, environment, and output never reach the browser. */
export function ProcessTable({
  processes,
}: {
  readonly processes: readonly HostedProcess[]
}): ReactNode {
  return (
    <table className="ops-table">
      <caption className="visually-hidden">Processes this Runtime hosts</caption>
      <thead>
        <tr>
          <th scope="col">Process</th>
          <th scope="col">Creator</th>
          <th scope="col">State</th>
          <th scope="col">Outcome</th>
        </tr>
      </thead>
      <tbody>
        {processes.map((process) => (
          <tr key={process.process_id}>
            <th className="ops-table__id" scope="row">
              {process.name ?? shortId(process.process_id)}
            </th>
            <td className="ops-table__id">{shortId(process.creator_endpoint_id)}</td>
            <td>{process.archived ? `${process.state} (archived)` : process.state}</td>
            <td>{processOutcome(process)}</td>
          </tr>
        ))}
      </tbody>
    </table>
  )
}
