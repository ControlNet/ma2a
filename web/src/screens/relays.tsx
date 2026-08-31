import { type FormEvent, type ReactNode, useState } from "react"
import type { PrivateRelayConfiguration } from "../api/mutations"
import { CodeValue, EmptyState, PageHeader, Section, StatusText } from "../components/primitives"
import { PendingRuntime } from "../components/runtime-status"
import type { RuntimeActions } from "../runtime-actions"
import type { RelayStatus, RuntimeViewData } from "../view-model"

function relayTone(status: RelayStatus): "neutral" | "success" | "warning" | "error" {
  switch (status) {
    case "eligible":
      return "success"
    case "disabled":
      return "warning"
  }
}

export function RelaysScreen({
  runtime,
  actions,
}: {
  readonly runtime: RuntimeViewData | undefined
  readonly actions: RuntimeActions | undefined
}): ReactNode {
  const [message, setMessage] = useState<string | undefined>()
  const [relayMode, setRelayMode] = useState<PrivateRelayConfiguration["mode"]>("native_tls")
  const privateRelay = (event: FormEvent<HTMLFormElement>): void => {
    event.preventDefault()
    const values = new FormData(event.currentTarget)
    const listen = values.get("listen")
    const publicUrl = values.get("public-url")
    const servedSpaces = values.get("served-spaces")
    const certificatePath = values.get("certificate-path")
    const privateKeyPath = values.get("private-key-path")
    if (
      typeof listen !== "string" ||
      typeof publicUrl !== "string" ||
      typeof servedSpaces !== "string" ||
      typeof certificatePath !== "string" ||
      typeof privateKeyPath !== "string" ||
      actions === undefined
    )
      return
    const servedSpaceIds = servedSpaces
      .split(/[\s,]+/u)
      .map((value) => value.trim())
      .filter((value) => value.length > 0)
    const configuration =
      relayMode === "native_tls"
        ? {
            mode: relayMode,
            listen,
            publicUrl,
            servedSpaceIds,
            certificatePath,
            privateKeyPath,
          }
        : {
            mode: relayMode,
            listen,
            publicUrl,
            servedSpaceIds,
            certificatePath: null,
            privateKeyPath: null,
          }
    setMessage("Configuring Private Relay...")
    void actions.configurePrivateRelay(configuration).then(
      (relay) => setMessage(`Private Relay configured at ${relay.host}:${relay.port}.`),
      () => setMessage("Private Relay configuration failed."),
    )
  }
  const publicRelay = (event: FormEvent<HTMLFormElement>): void => {
    event.preventDefault()
    const url = new FormData(event.currentTarget).get("url")
    if (typeof url !== "string" || actions === undefined) return
    setMessage("Configuring Public fallback...")
    void actions.configurePublicRelay(url).then(
      () => setMessage("Public fallback configuration updated."),
      () => setMessage("Public fallback configuration failed."),
    )
  }
  return (
    <div className="page-stack">
      <PageHeader
        description="Desired compatible candidates remain separate from the effective path observed by Iroh."
        title="Relays and reachability"
      />
      <div className="operation-grid">
        <form className="form-stack" onSubmit={privateRelay}>
          <h2>Private Provider</h2>
          <label htmlFor="private-mode">TLS termination</label>
          <select
            id="private-mode"
            name="mode"
            onChange={(event) =>
              setRelayMode(
                event.currentTarget.value === "native_tls" ? "native_tls" : "external_termination",
              )
            }
            value={relayMode}
          >
            <option value="native_tls">Native embedded TLS</option>
            <option value="external_termination">Optional external termination</option>
          </select>
          <label htmlFor="private-listen">Listen address</label>
          <input id="private-listen" name="listen" required />
          <label htmlFor="private-public-url">Public HTTPS URL</label>
          <input
            id="private-public-url"
            name="public-url"
            pattern="https://.*"
            required
            type="url"
          />
          <label htmlFor="private-spaces">Served Space IDs</label>
          <textarea id="private-spaces" name="served-spaces" required rows={3} />
          <p className="field-help">Separate multiple Space IDs with commas or line breaks.</p>
          <label htmlFor="private-certificate">TLS certificate path</label>
          <input
            id="private-certificate"
            name="certificate-path"
            required={relayMode === "native_tls"}
          />
          <label htmlFor="private-key">TLS private key path</label>
          <input id="private-key" name="private-key-path" required={relayMode === "native_tls"} />
          <p className="field-help">
            Native TLS requires both local paths. External termination ignores both fields.
          </p>
          <button disabled={actions === undefined} type="submit">
            Configure Private
          </button>
        </form>
        <form className="form-stack" onSubmit={publicRelay}>
          <h2>Public Fallback</h2>
          <label htmlFor="public-url">HTTPS relay URL</label>
          <input id="public-url" name="url" pattern="https://.*" required type="url" />
          <p className="field-help">Fallback is explicit; MA2A never promotes it automatically.</p>
          <button disabled={actions === undefined} type="submit">
            Configure Public
          </button>
        </form>
      </div>
      {message === undefined ? null : <p role="status">{message}</p>}
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
                  <caption>Compatible candidates supplied to Iroh</caption>
                  <thead>
                    <tr>
                      <th scope="col">Role</th>
                      <th scope="col">Provider Endpoint</th>
                      <th scope="col">State</th>
                    </tr>
                  </thead>
                  <tbody>
                    {runtime.relays.map((relay) => (
                      <tr key={relay.endpointId}>
                        <th scope="row">
                          {relay.kind === "private" ? "Private Relay" : "Public fallback"}
                        </th>
                        <td>
                          <CodeValue>{relay.endpointId}</CodeValue>
                        </td>
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
          <Section title="Iroh-observed effective state">
            <dl className="detail-list">
              <div>
                <dt>Private relay online</dt>
                <dd>{runtime.observedRelayState.private_relay_online ? "yes" : "no"}</dd>
              </div>
              <div>
                <dt>Public relay online</dt>
                <dd>{runtime.observedRelayState.public_relay_online ? "yes" : "no"}</dd>
              </div>
              <div>
                <dt>Preferred/home/path</dt>
                <dd>{runtime.endpoint.observedPath}; selected by Iroh, not MA2A</dd>
              </div>
            </dl>
          </Section>
        </div>
      )}
    </div>
  )
}
