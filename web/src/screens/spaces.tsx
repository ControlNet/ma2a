import { type ReactNode, useState } from "react"

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
  const [message, setMessage] = useState<string | undefined>()
  const run = (operation: () => Promise<void>, success: string): void => {
    setMessage("Working…")
    void operation().then(
      () => setMessage(success),
      () => setMessage("The Runtime rejected the request."),
    )
  }
  const disabled = actions === undefined
  const spaces = runtime?.spaces ?? []
  return (
    <Inspector eyebrow="Operations" title="Space actions">
      <Card label="Create a Space">
        <Form
          disabled={disabled}
          label="Create a Space"
          onSubmit={(data) => {
            if (actions === undefined) return
            run(() => actions.createSpace(text(data, "name")), "Space created by the Runtime.")
          }}
          submitLabel="Create"
          {...(message === undefined ? {} : { status: message })}
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
              () =>
                actions.createInvitation(
                  text(data, "space-id"),
                  Number(text(data, "ttl-ms")),
                  text(data, "output-path"),
                ),
              "Ticket written by the Runtime. Its secret never reaches this browser.",
            )
          }}
          submitLabel="Create invite"
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
              () => actions.revokeEndpoint(text(data, "space-id"), text(data, "endpoint-id")),
              "Member removed. The revocation applies to this Space only.",
            )
          }}
          submitLabel="Remove"
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
