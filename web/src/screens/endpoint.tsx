import { type ReactNode, useState } from "react"

import { CodeValue, PageHeader, Section, StatusText } from "../components/primitives"
import { PendingRuntime } from "../components/runtime-status"
import type { RuntimeViewData } from "../view-model"

export function EndpointScreen({
  runtime,
}: {
  readonly runtime: RuntimeViewData | undefined
}): ReactNode {
  const [copyResult, setCopyResult] = useState<
    | {
        readonly endpointId: string
        readonly label: "Copied" | "Copy failed"
      }
    | undefined
  >()

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
            <div className="table-wrap">
              <table className="connection-table">
                <caption>Latest observed peer connections</caption>
                <thead>
                  <tr>
                    <th scope="col">Peer Endpoint</th>
                    <th scope="col">State</th>
                    <th scope="col">Iroh path</th>
                    <th scope="col">RTT</th>
                  </tr>
                </thead>
                <tbody>
                  {runtime.peerConnections.length === 0 ? (
                    <tr>
                      <td colSpan={4}>No peer connection has been observed yet.</td>
                    </tr>
                  ) : (
                    runtime.peerConnections.map((peer) => (
                      <tr key={peer.endpointId}>
                        <td data-label="Peer Endpoint">
                          <div className="peer-endpoint-value">
                            <CodeValue>
                              {peer.endpointId.slice(0, 6)}…{peer.endpointId.slice(-4)}
                            </CodeValue>
                            <button
                              aria-label={`Copy full Endpoint ID ${peer.endpointId}`}
                              className="button-secondary peer-endpoint-copy"
                              onClick={() => {
                                void navigator.clipboard.writeText(peer.endpointId).then(
                                  () =>
                                    setCopyResult({ endpointId: peer.endpointId, label: "Copied" }),
                                  () =>
                                    setCopyResult({
                                      endpointId: peer.endpointId,
                                      label: "Copy failed",
                                    }),
                                )
                              }}
                              type="button"
                            >
                              Copy full ID
                            </button>
                          </div>
                          <span aria-live="polite" className="copy-feedback">
                            {copyResult?.endpointId === peer.endpointId ? copyResult.label : ""}
                          </span>
                        </td>
                        <td data-label="State">
                          <StatusText
                            tone={
                              peer.state === "connected"
                                ? "success"
                                : peer.state === "failed"
                                  ? "error"
                                  : "warning"
                            }
                          >
                            {peer.state}
                          </StatusText>
                        </td>
                        <td data-label="Iroh path">{peer.path}</td>
                        <td data-label="RTT">
                          {peer.rttMs === undefined ? "Not observed" : `${peer.rttMs} ms`}
                        </td>
                      </tr>
                    ))
                  )}
                </tbody>
              </table>
            </div>
          </Section>
          <aside className="status-aside" aria-label="Endpoint state">
            <h2>Observed state</h2>
            <dl className="detail-list">
              <div>
                <dt>Status</dt>
                <dd>
                  <StatusText
                    tone={
                      runtime.endpoint.status === "active"
                        ? "success"
                        : runtime.endpoint.status === "offline"
                          ? "error"
                          : "warning"
                    }
                  >
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
              <div>
                <dt>Runtime version</dt>
                <dd>{runtime.runtimeVersion}</dd>
              </div>
            </dl>
          </aside>
        </div>
      )}
    </div>
  )
}
