import type { ReactNode } from "react"

import { CodeValue, EmptyState, PageHeader, Section, StatusText } from "../components/primitives"
import { PendingRuntime } from "../components/runtime-status"
import type { RelayStatus, RuntimeViewData } from "../view-model"

function relayTone(status: RelayStatus): "neutral" | "success" | "warning" | "error" {
  switch (status) {
    case "selected":
      return "success"
    case "eligible":
      return "neutral"
    case "disabled":
      return "warning"
    case "expired":
      return "error"
  }
}

export function RelaysScreen({
  runtime,
}: {
  readonly runtime: RuntimeViewData | undefined
}): ReactNode {
  return (
    <div className="page-stack">
      <PageHeader
        description="Desired compatible candidates remain separate from the effective path observed by Iroh."
        title="Relays and reachability"
      />
      {runtime === undefined ? (
        <PendingRuntime />
      ) : (
        <div className="page-stack">
          <Section description={runtime.reachability.detail} title="Target-specific reachability">
            <div className="reachability-line">
              <StatusText tone={runtime.reachability.status === "degraded" ? "warning" : "neutral"}>
                {runtime.reachability.status}
              </StatusText>
              <strong>{runtime.reachability.path}</strong>
            </div>
          </Section>
          <Section
            description="Private infrastructure and optional Public fallback are shown as distinct roles."
            title="Relay candidates"
          >
            {runtime.relays.length === 0 ? (
              <EmptyState
                description="No compatible Private Relay or enabled Public fallback is present in this snapshot."
                title="No relay candidates"
              />
            ) : (
              <div className="table-wrap">
                <table>
                  <caption>Configured and observed relay candidates</caption>
                  <thead>
                    <tr>
                      <th scope="col">Role</th>
                      <th scope="col">Relay URL</th>
                      <th scope="col">Compatibility</th>
                      <th scope="col">State</th>
                    </tr>
                  </thead>
                  <tbody>
                    {runtime.relays.map((relay) => (
                      <tr key={relay.id}>
                        <th scope="row">
                          {relay.kind === "private" ? "Private Relay" : "Public fallback"}
                        </th>
                        <td>
                          <CodeValue>{relay.url}</CodeValue>
                        </td>
                        <td>{relay.compatibility}</td>
                        <td>
                          <StatusText tone={relayTone(relay.status)}>{relay.status}</StatusText>
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
          </Section>
        </div>
      )}
    </div>
  )
}
