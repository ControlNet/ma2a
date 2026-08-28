import type { ReactNode } from "react"

import { EmptyState, PageHeader, Section, StatusText } from "../components/primitives"
import { PendingRuntime, StateRail } from "../components/runtime-status"
import type { RuntimeViewData } from "../view-model"

function endpointTone(
  status: RuntimeViewData["endpoint"]["status"],
): "success" | "warning" | "error" {
  switch (status) {
    case "active":
      return "success"
    case "degraded":
      return "warning"
    case "offline":
      return "error"
  }
}

export function OverviewScreen({
  runtime,
}: {
  readonly runtime: RuntimeViewData | undefined
}): ReactNode {
  return (
    <div className="page-stack">
      <PageHeader
        description="Authoritative local identity, Space membership, relay posture, and recent Echo outcomes."
        title="Runtime overview"
      />
      {runtime === undefined ? (
        <PendingRuntime />
      ) : (
        <>
          <StateRail runtime={runtime} />
          <div className="overview-columns">
            <Section title="Current posture">
              <dl className="detail-list">
                <div>
                  <dt>Endpoint</dt>
                  <dd>
                    <StatusText tone={endpointTone(runtime.endpoint.status)}>
                      {runtime.endpoint.status}
                    </StatusText>
                  </dd>
                </div>
                <div>
                  <dt>Reachability</dt>
                  <dd>{runtime.reachability.path}</dd>
                </div>
                <div>
                  <dt>Relay candidates</dt>
                  <dd>{runtime.relays.length}</dd>
                </div>
              </dl>
            </Section>
            <Section title="Recent Echo">
              {runtime.echoHistory.length === 0 ? (
                <EmptyState
                  description="Run an Echo test against an Endpoint to record a bounded result summary."
                  title="No Echo history"
                />
              ) : (
                <ul className="compact-list">
                  {runtime.echoHistory.slice(0, 3).map((echo) => (
                    <li key={echo.requestId}>
                      <span>{echo.target}</span>
                      <StatusText tone={echo.status === "echoed" ? "success" : "error"}>
                        {echo.status}
                      </StatusText>
                    </li>
                  ))}
                </ul>
              )}
            </Section>
          </div>
        </>
      )}
    </div>
  )
}
