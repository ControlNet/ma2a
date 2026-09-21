import type { ReactNode } from "react"

import { EmptyState, PendingSnapshot } from "../components/feedback"
import { Inspector } from "../components/inspector"
import { Button, Card, Identifier, Pill, Section } from "../components/ui"
import type { RuntimeActions } from "../runtime-actions"
import type { RuntimeViewData } from "../view-model"
import { MeterList } from "../viz/meter-list"
import { ObservationTrack } from "../viz/observation-track"
import { StackBar } from "../viz/stack-bar"
import { toneColor } from "../viz/tone"
import { LIMITS, observationTrack, pathParts, peerList, shortId } from "./derive"

export function PeersScreen({
  runtime,
  selected,
  onSelect,
}: {
  readonly runtime: RuntimeViewData | undefined
  readonly selected: string | undefined
  readonly onSelect: (id: string) => void
}): ReactNode {
  if (runtime === undefined) return <PendingSnapshot />
  const peers = peerList(runtime)
  return (
    <Section
      description="Peer Endpoints from signed membership, with Iroh observations where available."
      title="Peers"
    >
      {runtime.spaces.some((space) => space.members === undefined) ? (
        <p role="status">
          {runtime.spaces.some((space) => space.membersError)
            ? "Some signed members could not be loaded. Refresh to retry."
            : "Loading signed members. The peer list is incomplete."}
        </p>
      ) : null}
      <Card label="This Endpoint">
        <div className="cluster">
          <Pill tone={runtime.endpoint.status === "active" ? "direct" : "failed"}>
            {runtime.endpoint.status}
          </Pill>
          <Pill tone="none">aggregate path: {runtime.endpoint.observedPath}</Pill>
          <Pill tone="accent">runtime {runtime.runtimeVersion}</Pill>
        </div>
      </Card>
      {peers.length === 0 ? (
        <EmptyState title="No peer Endpoint known yet">
          Members of a Space appear here whether or not a path has been observed.
        </EmptyState>
      ) : (
        <ul className="peer-list">
          {peers.map((peer) => (
            <li key={peer.id}>
              <button
                aria-pressed={selected === peer.id}
                className="peer"
                onClick={() => onSelect(peer.id)}
                style={{ borderInlineStartColor: toneColor(peer.tone) }}
                type="button"
              >
                <span className="peer__id">{shortId(peer.id)}</span>
                <span className="peer__state">{peer.state}</span>
                <span className="peer__badge" style={{ color: toneColor(peer.tone) }}>
                  {peer.badge}
                </span>
              </button>
            </li>
          ))}
        </ul>
      )}
    </Section>
  )
}

export function PeerInspector({
  runtime,
  selected,
  actions,
  onSync,
  message,
}: {
  readonly runtime: RuntimeViewData | undefined
  readonly selected: string | undefined
  readonly actions: RuntimeActions | undefined
  readonly onSync: (endpointId: string) => void
  readonly message: string | undefined
}): ReactNode {
  if (runtime === undefined) {
    return (
      <Inspector eyebrow="Peer" title="No snapshot">
        <p className="field__help">Waiting for the Runtime.</p>
      </Inspector>
    )
  }
  const peer = peerList(runtime).find((candidate) => candidate.id === selected)
  if (peer === undefined) {
    return (
      <Inspector eyebrow="Peers" title="Nothing selected">
        <Card label="Observed paths">
          <StackBar caption="Observed paths by peer" parts={pathParts(runtime)} />
        </Card>
        <Card label="Bounded capacity">
          <MeterList
            label="Connection capacity"
            meters={[
              {
                label: "Owned connections",
                fraction: runtime.peerConnections.length / LIMITS.connections,
                value: `${runtime.peerConnections.length} / ${LIMITS.connections}`,
                tone: "accent",
              },
            ]}
          />
        </Card>
      </Inspector>
    )
  }
  return (
    <Inspector
      actions={
        <Button disabled={actions === undefined} onClick={() => onSync(peer.id)} variant="primary">
          Sync now
        </Button>
      }
      eyebrow="Peer Endpoint"
      title={shortId(peer.id)}
    >
      <Identifier value={peer.id} />
      <Card label="Retained observations">
        {(() => {
          const track = observationTrack(runtime, peer.id)
          return track.length === 0 ? (
            <p className="field__help">No observation is retained for this peer yet.</p>
          ) : (
            <ObservationTrack observations={track} />
          )
        })()}
        <p className="field__help">
          Cleared at restart. Numbers are the round-trip estimate in ms.
        </p>
      </Card>
      <Card label="Observed">
        <div className="cluster">
          <Pill filled tone={peer.tone}>
            {peer.state}
          </Pill>
          <Pill tone={peer.tone}>{peer.badge}</Pill>
        </div>
        <p className="field__help">The authorizing Space is never projected.</p>
      </Card>
      {message === undefined ? null : (
        <p className="field__help" role="status">
          {message}
        </p>
      )}
    </Inspector>
  )
}
