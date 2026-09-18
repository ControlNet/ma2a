import type { ReactNode } from "react"

import { type Tone, toneColor, toneDash } from "./tone"

export type Lane = {
  readonly spaceId: string
  readonly name: string
  readonly memberCount: number
}

export type ObservedPeer = {
  readonly id: string
  readonly shortId: string
  readonly state: string
  readonly tone: Tone
  readonly badge: string
}

function Chip({
  peer,
  selected,
  onSelect,
}: {
  readonly peer: ObservedPeer
  readonly selected: boolean
  readonly onSelect: ((id: string) => void) | undefined
}): ReactNode {
  const style = {
    borderColor: toneColor(peer.tone),
    borderStyle: toneDash(peer.tone) === undefined ? "solid" : "dashed",
  }
  const body = (
    <>
      <span className="chip__id">{peer.shortId}</span>
      <span className="chip__name">{peer.state}</span>
      <span className="chip__badge" style={{ color: toneColor(peer.tone) }}>
        {peer.badge}
      </span>
    </>
  )
  if (onSelect === undefined) {
    return (
      <span className="chip" style={style}>
        {body}
      </span>
    )
  }
  return (
    <button
      aria-pressed={selected}
      className="chip chip--interactive"
      onClick={() => onSelect(peer.id)}
      style={style}
      type="button"
    >
      {body}
    </button>
  )
}

/**
 * Two bands that must never be read as one. Lanes are Spaces: signed, durable
 * authorization, drawn in neutral because membership is not a status. Peers are
 * what Iroh currently observes, drawn in an observed tone. The Runtime does not
 * publish which peer belongs to which Space, and the layout says so out loud.
 */
export function LaneMap({
  endpointId,
  lanes,
  peers,
  selected,
  onSelect,
}: {
  readonly endpointId: string
  readonly lanes: readonly Lane[]
  readonly peers: readonly ObservedPeer[]
  readonly selected?: string
  readonly onSelect?: (id: string) => void
}): ReactNode {
  return (
    <div className="lane-map">
      <div className="lane-map__self">
        <span className="lane-map__eyebrow">This Endpoint</span>
        <span className="lane-map__id">{endpointId}</span>
      </div>
      <div className="lane-map__bands">
        <section className="band band--signed">
          <h2 className="band__title">Signed authorization</h2>
          <p className="band__note">
            One lane per Space, each authorizing on its own. This API publishes a member count, not
            member identities.
          </p>
          {lanes.length === 0 ? (
            <div className="lane-map__empty">
              <strong>No Space, so no lane to draw</strong>
              <p>
                A zero-Space Runtime advertises only <code>ma2a/enrollment/1</code>. Echo, control,
                metadata and relay are refused before any stream opens.
              </p>
            </div>
          ) : (
            <ul className="lane-map__lanes">
              {lanes.map((lane) => (
                <li className="lane" key={lane.spaceId}>
                  <span className="lane__name">{lane.name}</span>
                  <span className="lane__space">{lane.spaceId}</span>
                  <span className="lane__meta">
                    {lane.memberCount} {lane.memberCount === 1 ? "member" : "members"}
                  </span>
                </li>
              ))}
            </ul>
          )}
        </section>
        <section className="band band--observed">
          <h2 className="band__title">Observed transport</h2>
          <p className="band__note">
            What Iroh reports right now. Cleared on restart, and never an authorization fact.
          </p>
          {peers.length === 0 ? (
            <div className="lane-map__empty lane-map__empty--quiet">
              <strong>No peer observed yet</strong>
              <p>Members stay fully authorized whether or not a path has ever been seen.</p>
            </div>
          ) : (
            <div className="lane__members">
              {peers.map((peer) => (
                <Chip
                  key={peer.id}
                  onSelect={onSelect}
                  peer={peer}
                  selected={selected === peer.id}
                />
              ))}
            </div>
          )}
        </section>
      </div>
    </div>
  )
}
