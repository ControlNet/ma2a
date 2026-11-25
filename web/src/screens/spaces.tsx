import { type ReactNode, useRef, useState } from "react"

import { mutationFailureText } from "../api/mutations"
import { EmptyState, PendingSnapshot } from "../components/feedback"
import { Field, Form, text } from "../components/form"
import { Inspector } from "../components/inspector"
import { Card, Pill, Section } from "../components/ui"
import type { RuntimeActions } from "../runtime-actions"
import type { RuntimeViewData } from "../view-model"
import { LIMITS, shortId } from "./derive"

export function SpacesScreen({
  runtime,
}: {
  readonly runtime: RuntimeViewData | undefined
}): ReactNode {
  if (runtime === undefined) return <PendingSnapshot />
  return (
    <Section title="Spaces">
      {runtime.spaces.length === 0 ? (
        <EmptyState title="No Space yet">
          Create one here, or accept an invitation ticket from the trusted terminal with
          <code> ma2a space accept</code>. Accepting has no browser route.
        </EmptyState>
      ) : (
        <div className="split">
          {runtime.spaces.map((space) => (
            <Card key={space.id} label={space.name}>
              <code className="identifier__value">{space.id}</code>
              {space.revokedCount === 0 ? null : (
                <div className="cluster">
                  <Pill tone="failed">{space.revokedCount} revoked, carried forward</Pill>
                </div>
              )}
              <div>
                <span className="eyebrow">
                  Signed members · {space.memberCount} of {LIMITS.members}
                </span>
                {space.members === undefined ? (
                  <p role="status">
                    {space.membersError
                      ? "Members could not be loaded. Refresh to retry."
                      : "Loading signed members…"}
                  </p>
                ) : (
                  <ul className="members">
                    {space.members.map((member) => (
                      <li className="member" key={member.endpointId}>
                        <span className="member__id">{shortId(member.endpointId)}</span>
                        <span className="member__label">{member.label}</span>
                        <span className="member__caps">
                          {member.echo ? <Pill tone="direct">echo</Pill> : null}
                          {member.grants.map((grant) => (
                            <span className="grant" key={grant}>
                              {grant}
                            </span>
                          ))}
                        </span>
                      </li>
                    ))}
                  </ul>
                )}
              </div>
            </Card>
          ))}
        </div>
      )}
    </Section>
  )
}

export function SpacesInspector({
  runtime,
  actions,
}: {
  readonly runtime: RuntimeViewData | undefined
  readonly actions: RuntimeActions | undefined
}): ReactNode {
  const [busy, setBusy] = useState(false)
  const busyRef = useRef(false)
  const [status, setStatus] = useState<{
    readonly create?: string
    readonly invite?: string
    readonly revoke?: string
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
  const disabled = actions === undefined || busy
  const spaces = runtime?.spaces ?? []
  return (
    <Inspector eyebrow="Operations" title="Space actions">
      <Card label="Create a Space">
        <Form
          disabled={disabled}
          label="Create a Space"
          onSubmit={(data) => {
            if (actions === undefined) return
            run(
              "create",
              () => actions.createSpace(text(data, "name")),
              "Space created by the Runtime.",
              "No answer came back, so the Space may already exist. Create it again with the same name to finish this request without a second Space.",
            )
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
