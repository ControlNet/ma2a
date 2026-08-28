import type { ReactNode } from "react"

import { EmptyState, PageHeader, Section, StatusText } from "../components/primitives"
import { PendingRuntime } from "../components/runtime-status"
import type { EchoStatus, RuntimeViewData } from "../view-model"

function echoTone(status: EchoStatus): "success" | "error" {
  switch (status) {
    case "echoed":
      return "success"
    case "denied":
    case "failed":
      return "error"
  }
}

export function EchoScreen({
  runtime,
}: {
  readonly runtime: RuntimeViewData | undefined
}): ReactNode {
  return (
    <div className="page-stack">
      <PageHeader
        description="Address one Endpoint directly. Authorization remains receiver-derived and Space-private."
        title="Echo Test"
      />
      {runtime === undefined ? (
        <PendingRuntime />
      ) : (
        <div className="evidence-layout">
          <Section title="Request">
            <form className="form-stack">
              <label htmlFor="echo-target">Target Endpoint ID</label>
              <input
                aria-describedby="echo-target-help"
                id="echo-target"
                name="target"
                placeholder="Endpoint ID"
                type="text"
              />
              <p className="field-help" id="echo-target-help">
                No Space selector is used. The receiver determines whether one complete shared Space
                authorizes Echo.
              </p>
              <label htmlFor="echo-payload">Payload</label>
              <textarea defaultValue="hello" id="echo-payload" name="payload" rows={4} />
              <button disabled type="submit">
                Runtime API required
              </button>
            </form>
          </Section>
          <aside className="status-aside" aria-label="Recent Echo outcomes">
            <h2>Recent outcomes</h2>
            {runtime.echoHistory.length === 0 ? (
              <EmptyState
                description="Only bounded status summaries appear here. Payloads and authorization details stay private."
                title="No Echo history"
              />
            ) : (
              <ul className="compact-list compact-list--stacked">
                {runtime.echoHistory.map((echo) => (
                  <li key={echo.requestId}>
                    <span>{echo.target}</span>
                    <span>{echo.path}</span>
                    <StatusText tone={echoTone(echo.status)}>{echo.status}</StatusText>
                  </li>
                ))}
              </ul>
            )}
          </aside>
        </div>
      )}
    </div>
  )
}
