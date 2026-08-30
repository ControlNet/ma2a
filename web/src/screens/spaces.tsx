import { type FormEvent, type ReactNode, useState } from "react"

import { CodeValue, EmptyState, PageHeader, StatusText } from "../components/primitives"
import { PendingRuntime } from "../components/runtime-status"
import type { RuntimeActions } from "../runtime-actions"
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
  actions,
}: {
  readonly runtime: RuntimeViewData | undefined
  readonly actions: RuntimeActions | undefined
}): ReactNode {
  const [message, setMessage] = useState<string | undefined>()
  const submit = (operation: () => Promise<void>, success: string): void => {
    setMessage("Working...")
    void operation().then(
      () => setMessage(success),
      () => setMessage("The Runtime rejected the request."),
    )
  }
  const create = (event: FormEvent<HTMLFormElement>): void => {
    event.preventDefault()
    const name = new FormData(event.currentTarget).get("name")
    if (typeof name === "string" && actions !== undefined) {
      submit(() => actions.createSpace(name), "Space created from the authoritative Runtime.")
      event.currentTarget.reset()
    }
  }
  const peerAction = (event: FormEvent<HTMLFormElement>, kind: "invite" | "revoke"): void => {
    event.preventDefault()
    const values = new FormData(event.currentTarget)
    const spaceId = values.get("space-id")
    const endpointId = values.get("endpoint-id")
    if (typeof spaceId !== "string" || typeof endpointId !== "string" || actions === undefined)
      return
    const operation = kind === "invite" ? actions.inviteEndpoint : actions.revokeEndpoint
    submit(
      () => operation(spaceId, endpointId),
      kind === "invite"
        ? "Endpoint invitation recorded. The Web API does not expose invite secret material."
        : "Endpoint revocation recorded.",
    )
  }
  return (
    <div className="page-stack">
      <PageHeader
        description="Private membership summaries for this Endpoint. Membership never changes its identity."
        title="Spaces"
      />
      <div className="operation-grid">
        <form className="form-stack" onSubmit={create}>
          <h2>Create Space</h2>
          <label htmlFor="space-name">Local label</label>
          <input id="space-name" maxLength={128} name="name" required />
          <button disabled={actions === undefined} type="submit">
            Create
          </button>
        </form>
        <form className="form-stack" onSubmit={(event) => peerAction(event, "invite")}>
          <h2>Invite Endpoint</h2>
          <label htmlFor="invite-space">Space ID</label>
          <input id="invite-space" name="space-id" pattern="[0-9a-f]{64}" required />
          <label htmlFor="invite-endpoint">Endpoint ID</label>
          <input id="invite-endpoint" name="endpoint-id" pattern="[0-9a-f]{64}" required />
          <button disabled={actions === undefined} type="submit">
            Invite
          </button>
        </form>
        <form className="form-stack" onSubmit={(event) => peerAction(event, "revoke")}>
          <h2>Revoke Endpoint</h2>
          <label htmlFor="revoke-space">Space ID</label>
          <input id="revoke-space" name="space-id" pattern="[0-9a-f]{64}" required />
          <label htmlFor="revoke-endpoint">Endpoint ID</label>
          <input id="revoke-endpoint" name="endpoint-id" pattern="[0-9a-f]{64}" required />
          <button className="button-danger" disabled={actions === undefined} type="submit">
            Revoke
          </button>
        </form>
      </div>
      {message === undefined ? null : <p role="status">{message}</p>}
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
                <th scope="col">Member detail</th>
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
                  <td>Summary only; member identities are not exposed by this API.</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  )
}
