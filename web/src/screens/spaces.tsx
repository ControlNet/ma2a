import type { ReactNode } from "react"

import { CodeValue, EmptyState, PageHeader, StatusText } from "../components/primitives"
import { PendingRuntime } from "../components/runtime-status"
import type { RuntimeViewData, SyncState } from "../view-model"

function syncTone(sync: SyncState): "success" | "warning" | "error" {
  switch (sync) {
    case "current":
      return "success"
    case "catching-up":
      return "warning"
    case "stalled":
      return "error"
  }
}

export function SpacesScreen({
  runtime,
}: {
  readonly runtime: RuntimeViewData | undefined
}): ReactNode {
  return (
    <div className="page-stack">
      <PageHeader
        description="Private membership summaries for this Endpoint. Membership never changes its identity."
        title="Spaces"
      />
      {runtime === undefined ? (
        <PendingRuntime />
      ) : runtime.spaces.length === 0 ? (
        <EmptyState
          description="Create or redeem a Space from the trusted Runtime interfaces when available."
          title="No Spaces yet"
        />
      ) : (
        <div className="table-wrap">
          <table>
            <caption>Active Space memberships</caption>
            <thead>
              <tr>
                <th scope="col">Space</th>
                <th scope="col">Space ID</th>
                <th scope="col">Members</th>
                <th scope="col">Control sync</th>
              </tr>
            </thead>
            <tbody>
              {runtime.spaces.map((space) => (
                <tr key={space.id}>
                  <th scope="row">{space.name}</th>
                  <td>
                    <CodeValue>{space.id}</CodeValue>
                  </td>
                  <td>{space.memberCount}</td>
                  <td>
                    <StatusText tone={syncTone(space.sync)}>{space.sync}</StatusText>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  )
}
