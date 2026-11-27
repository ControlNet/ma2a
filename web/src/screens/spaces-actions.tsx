import { type ReactNode, useRef, useState } from "react"

import { mutationFailureText } from "../api/mutations"
import type { PendingSpaceCreation } from "../api/pending-space"
import { Field, Form, text } from "../components/form"
import { Inspector } from "../components/inspector"
import { Button, Card } from "../components/ui"
import type { RuntimeActions } from "../runtime-actions"
import type { RuntimeViewData } from "../view-model"
import { shortId } from "./derive"

export function SpacesInspector({
  runtime,
  actions,
  pendingSpaceCreation,
}: {
  readonly runtime: RuntimeViewData | undefined
  readonly actions: RuntimeActions | undefined
  readonly pendingSpaceCreation?: PendingSpaceCreation | undefined
}): ReactNode {
  const [busy, setBusy] = useState(false)
  const busyRef = useRef(false)
  const [status, setStatus] = useState<{
    readonly create?: string | undefined
    readonly invite?: string | undefined
    readonly revoke?: string | undefined
  }>({})
  // `unknown` is shown when no answer came back, so the request may have taken effect.
  const run = (
    slot: "create" | "invite" | "revoke",
    operation: () => Promise<void>,
    success: string,
    unknown: string,
  ): void => {
    if (busyRef.current) return
    busyRef.current = true
    setBusy(true)
    setStatus((current) => ({ ...current, [slot]: "Working…" }))
    void operation()
      .then(
        () => setStatus((current) => ({ ...current, [slot]: success })),
        (error: unknown) =>
          setStatus((current) => ({
            ...current,
            [slot]: mutationFailureText(error, unknown, "The Runtime rejected the request."),
          })),
      )
      .finally(() => {
        busyRef.current = false
        setBusy(false)
      })
  }
  const createSpace = (name: string): void => {
    if (actions === undefined) return
    run(
      "create",
      () => actions.createSpace(name),
      "Space created by the Runtime.",
      "No answer came back, so the Space may already exist. Create it again with the same name to finish this request without a second Space.",
    )
  }
  const disabled = actions === undefined || runtime === undefined || busy
  const spaces = runtime?.spaces ?? []
  return (
    <Inspector eyebrow="Operations" title="Space actions">
      <Card label="Create a Space">
        <Form
          disabled={disabled}
          label="Create a Space"
          onSubmit={(data) => {
            const name = text(data, "name")
            if (pendingSpaceCreation !== undefined && name !== pendingSpaceCreation.name) {
              setStatus((current) => ({
                ...current,
                create: `Unresolved request for “${pendingSpaceCreation.name}”. Retry it or choose Start new creation first.`,
              }))
              return
            }
            createSpace(name)
          }}
          submitLabel="Create"
          {...(status.create === undefined ? {} : { status: status.create })}
        >
          <Field
            help="Shared metadata signed into the Space. Every member reads the same name, and names are not unique."
            id="space-name"
            label="Name"
          >
            <input id="space-name" maxLength={128} name="name" required />
          </Field>
        </Form>
        {pendingSpaceCreation === undefined ? null : (
          <div className="stack-4" role="status">
            <p className="field__help">
              Unresolved request for “{pendingSpaceCreation.name}”. Retry it to finish without a
              second Space, or start a new creation to use a fresh request ID.
            </p>
            <div className="cluster">
              <Button
                disabled={disabled}
                onClick={() => createSpace(pendingSpaceCreation.name)}
                variant="primary"
              >
                Retry pending creation
              </Button>
              <Button
                disabled={disabled}
                onClick={() => {
                  actions?.discardPendingSpaceCreation(pendingSpaceCreation.requestId)
                  setStatus((current) => ({ ...current, create: undefined }))
                }}
              >
                Start new creation
              </Button>
            </div>
          </div>
        )}
      </Card>
      <Card label="Invite to a Space">
        <Form
          disabled={disabled}
          label="Invite to a Space"
          onSubmit={(data) => {
            if (actions === undefined) return
            run(
              "invite",
              () =>
                actions.createInvitation(
                  text(data, "space-id"),
                  Number(text(data, "ttl-ms")),
                  text(data, "output-path"),
                ),
              "Ticket written by the Runtime. Its secret never reaches this browser.",
              "No answer came back, so the ticket may already be written. Check the ticket file before trying again.",
            )
          }}
          submitLabel="Create invite"
          {...(status.invite === undefined ? {} : { status: status.invite })}
        >
          <Field id="invite-space" label="Space">
            <select id="invite-space" name="space-id" required>
              {spaces.map((space) => (
                <option key={space.id} value={space.id}>
                  {space.name} · {shortId(space.id)}
                </option>
              ))}
            </select>
          </Field>
          <Field
            help="Milliseconds on the Runtime clock, from 1 ms to 5 minutes."
            id="invite-ttl"
            label="Ticket lifetime"
          >
            <input
              defaultValue={300000}
              id="invite-ttl"
              max={300000}
              min={1}
              name="ttl-ms"
              required
              type="number"
            />
          </Field>
          <Field
            help="The ticket is a bearer secret written once with owner-only permissions."
            id="invite-path"
            label="Owner-only ticket file"
          >
            <input id="invite-path" maxLength={4096} name="output-path" required />
          </Field>
        </Form>
      </Card>
      <Card label="Remove a member">
        <Form
          danger
          disabled={disabled}
          label="Remove a member"
          onSubmit={(data) => {
            if (actions === undefined) return
            run(
              "revoke",
              () => actions.revokeEndpoint(text(data, "space-id"), text(data, "endpoint-id")),
              "Member removed. The revocation applies to this Space only.",
              "No answer came back, so the member may already be removed. Check the members before trying again.",
            )
          }}
          submitLabel="Remove"
          {...(status.revoke === undefined ? {} : { status: status.revoke })}
        >
          <Field id="revoke-space" label="Space">
            <select id="revoke-space" name="space-id" required>
              {spaces.map((space) => (
                <option key={space.id} value={space.id}>
                  {space.name}
                </option>
              ))}
            </select>
          </Field>
          <Field
            help="Records a Space-local revocation. Another shared Space can still authorize this peer."
            id="revoke-endpoint"
            label="Endpoint ID"
          >
            <input id="revoke-endpoint" name="endpoint-id" pattern="[0-9a-f]{64}" required />
          </Field>
        </Form>
      </Card>
    </Inspector>
  )
}
