import type { ReactNode } from "react"

import { EmptyState, PendingSnapshot } from "../components/feedback"
import { Card, Pill, Section } from "../components/ui"
import type { RuntimeViewData } from "../view-model"
import { LIMITS, shortId } from "./derive"

export { SpacesInspector } from "./spaces-actions"

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
