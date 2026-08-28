import type { ReactNode } from "react"

import { CodeValue, PageHeader, Section, StatusText } from "../components/primitives"
import { PendingRuntime } from "../components/runtime-status"
import type { RuntimeViewData } from "../view-model"

export function EndpointScreen({
  runtime,
}: {
  readonly runtime: RuntimeViewData | undefined
}): ReactNode {
  return (
    <div className="page-stack">
      <PageHeader
        description="One persistent Iroh Endpoint identity owned by this Runtime across every Space."
        title="Endpoint"
      />
      {runtime === undefined ? (
        <PendingRuntime />
      ) : (
        <div className="evidence-layout">
          <Section
            description="The Endpoint ID remains stable when Space memberships change."
            title="Local identity"
          >
            <div className="identity-block">
              <span>Endpoint ID</span>
              <CodeValue>{runtime.endpoint.id}</CodeValue>
            </div>
          </Section>
          <aside className="status-aside" aria-label="Endpoint state">
            <h2>Observed state</h2>
            <dl className="detail-list">
              <div>
                <dt>Status</dt>
                <dd>
                  <StatusText tone={runtime.endpoint.status === "active" ? "success" : "warning"}>
                    {runtime.endpoint.status}
                  </StatusText>
                </dd>
              </div>
              <div>
                <dt>Iroh path</dt>
                <dd>{runtime.endpoint.observedPath}</dd>
              </div>
              <div>
                <dt>Snapshot revision</dt>
                <dd>{runtime.revision}</dd>
              </div>
            </dl>
          </aside>
        </div>
      )}
    </div>
  )
}
