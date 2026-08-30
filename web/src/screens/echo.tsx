import { type FormEvent, type ReactNode, useState } from "react"

import { EmptyState, PageHeader, Section, StatusText } from "../components/primitives"
import { PendingRuntime } from "../components/runtime-status"
import type { RuntimeActions } from "../runtime-actions"
import type { RuntimeViewData } from "../view-model"

export function EchoScreen({
  runtime,
  actions,
}: {
  readonly runtime: RuntimeViewData | undefined
  readonly actions: RuntimeActions | undefined
}): ReactNode {
  const [result, setResult] = useState<string | undefined>()
  const submit = (event: FormEvent<HTMLFormElement>): void => {
    event.preventDefault()
    const values = new FormData(event.currentTarget)
    const endpointId = values.get("target")
    const payload = values.get("payload")
    if (typeof endpointId !== "string" || typeof payload !== "string" || actions === undefined)
      return
    setResult("Echo in progress...")
    void actions.echo(endpointId, payload).then(
      (reply) => setResult(`Echo reply from ${reply.target_endpoint_id}: ${reply.payload}`),
      () => setResult("Echo failed or was not authorized."),
    )
  }
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
            <form className="form-stack" onSubmit={submit}>
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
              <button disabled={actions === undefined} type="submit">
                Send Echo
              </button>
              {result === undefined ? null : <p role="status">{result}</p>}
            </form>
          </Section>
          <aside className="status-aside" aria-label="Recent Echo outcomes">
            <h2>Recent outcomes</h2>
            <dl className="detail-list">
              <div>
                <dt>Successes</dt>
                <dd>
                  <StatusText tone="success">{runtime.echoTotals.successes}</StatusText>
                </dd>
              </div>
              <div>
                <dt>Failures</dt>
                <dd>
                  <StatusText tone="error">{runtime.echoTotals.failures}</StatusText>
                </dd>
              </div>
            </dl>
            <EmptyState
              description="The authoritative snapshot retains bounded totals, not payload or target history."
              title="No detailed history retained"
            />
          </aside>
        </div>
      )}
    </div>
  )
}
