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
 * the peer Endpoints this Endpoint knows from signed membership, toned by
 * whatever Iroh has observed about reaching them. They are separate bands
 * on purpose: the snapshot does publish each Space's signed member set, and
 * keeping the two apart is what stops membership reading as reachability.
 */
export function LaneMap({
  lanes,
  peers,
  selected,
  onSelect,
}: {
  readonly lanes: readonly Lane[]
  readonly peers: readonly ObservedPeer[]
  readonly selected?: string
  readonly onSelect?: (id: string) => void
}): ReactNode {
  return (
    <div className="lane-map">
      <div className="lane-map__bands">
        <section className="band band--signed">
          <h2 className="band__title">Spaces</h2>
          {lanes.length === 0 ? (
            <div className="lane-map__empty">
              <strong>No Space yet</strong>
              <p>
                A zero-Space Runtime accepts only <code>ma2a/enrollment/1</code>.
              </p>
            </div>
          ) : (
            <ul className="lane-map__lanes">
              {lanes.map((lane) => (
                <li className="lane" key={lane.spaceId}>
                  <span className="lane__head">
                    <span className="lane__name">{lane.name}</span>
                    <span className="lane__space">{lane.spaceId}</span>
                  </span>
                  <span className="lane__meta">
                    {lane.memberCount} {lane.memberCount === 1 ? "member" : "members"}
                  </span>
                </li>
              ))}
            </ul>
          )}
        </section>
        <section className="band band--observed">
          <h2 className="band__title">Observed Peers</h2>
          {peers.length === 0 ? (
            <div className="lane-map__empty lane-map__empty--quiet">
              <strong>No peer observed yet</strong>
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
