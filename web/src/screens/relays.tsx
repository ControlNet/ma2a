import type { ReactNode } from "react"

import { EmptyState, PendingSnapshot } from "../components/feedback"
import { Card, Pill, Section } from "../components/ui"
import type { RuntimeViewData } from "../view-model"
import { CoverageMatrix } from "../viz/coverage-matrix"
import { MeterList } from "../viz/meter-list"
import { StateMachine } from "../viz/state-machine"
import { candidateCount, REACHABILITY_STATES, shortId } from "./derive"

export function RelaysScreen({
  runtime,
}: {
  readonly runtime: RuntimeViewData | undefined
}): ReactNode {
  if (runtime === undefined) return <PendingSnapshot />
  return (
    <Section
      description="MA2A filters candidates and hands them to Iroh. Iroh alone probes, selects a home and upgrades to direct."
      title="Relays"
    >
      <Card label="Reachability, as the Runtime names it">
        <StateMachine current={runtime.reachability.state} states={REACHABILITY_STATES} />
      </Card>
      <div className="split">
        <Card label="Relay advertisements and candidate selection">
          {runtime.privateRelayCandidates.length === 0 &&
          runtime.publicRelayFallbacks.length === 0 ? (
            <EmptyState title="No relay advertisement or fallback">
              No Private Relay advertisement and no configured Public Relay Fallback are present in
              this snapshot.
            </EmptyState>
          ) : (
            <>
              {runtime.privateRelayCandidates.length === 0 ? null : (
                <div>
                  <span className="eyebrow">Private Relay advertisements</span>
                  <div className="scroll-x">
                    <CoverageMatrix
                      caption="MA2A Private Relay coverage by Space"
                      rows={runtime.privateRelayCandidates.map((relay) => ({
                        id: relay.providerEndpointId,
                        name: relay.relayUrl,
                        detail: `provider ${shortId(relay.providerEndpointId)}`,
                        covers: runtime.spaces.map((space) =>
                          relay.coveredSpaceIds.includes(space.id),
                        ),
                        compatible: relay.homeCompatible,
                      }))}
                      spaces={runtime.spaces.map((space) => space.name)}
                    />
                  </div>
                  <p className="field__help">
                    Only a candidate covering every active Space is supplied to Iroh.
                  </p>
                </div>
              )}
              {runtime.publicRelayFallbacks.length === 0 ? null : (
                <div>
                  <span className="eyebrow">Public Relay Fallback</span>
                  <ul className="peer-list">
                    {runtime.publicRelayFallbacks.map((fallback) => (
                      <li key={fallback.relayUrl}>
                        <span className="peer peer--static">
                          <span className="peer__id">{fallback.relayUrl}</span>
                          <span className="peer__state">
                            external transport, no Endpoint identity
                          </span>
                          <Pill filled tone={fallback.observedConnected ? "direct" : "none"}>
                            {fallback.observedConnected ? "connected" : "not connected"}
                          </Pill>
                        </span>
                      </li>
                    ))}
                  </ul>
                </div>
              )}
            </>
          )}
          <div className="cluster">
            <span className="eyebrow">Candidates supplied to Iroh</span>
            <Pill filled tone={candidateCount(runtime) === 0 ? "none" : "direct"}>
              {`${candidateCount(runtime)} supplied`}
            </Pill>
          </div>
          <p className="field__help">
            A Private Relay is a role hosted by an MA2A Endpoint; a Public Relay Fallback is
            external infrastructure with no Endpoint identity. A supplied candidate is not a
            reachability guarantee.
          </p>
        </Card>
        <Card label="Iroh transport observation">
          <MeterList
            label="Iroh transport observation"
            meters={[
              {
                label: "Public Relay Fallback connected",
                fraction: runtime.observedRelayState.publicRelayConnected ? 1 : 0,
                value: runtime.observedRelayState.publicRelayConnected ? "yes" : "no",
                tone: runtime.observedRelayState.publicRelayConnected ? "direct" : "none",
              },
            ]}
          />
          <div className="cluster">
            <Pill tone="none">observed path: {runtime.endpoint.observedPath}</Pill>
          </div>
          <p className="field__help">
            Cleared at every restart. MA2A never picks Iroh&apos;s home relay.
          </p>
        </Card>
        <Card label="Local provider role">
          <MeterList
            label="Local provider role"
            meters={[
              {
                label: "Private Relay Provider running here",
                fraction: runtime.observedRelayState.privateRelayProviderRunning ? 1 : 0,
                value: runtime.observedRelayState.privateRelayProviderRunning ? "yes" : "no",
                tone: runtime.observedRelayState.privateRelayProviderRunning ? "accent" : "none",
              },
            ]}
          />
          <p className="field__help">
            A service this Runtime hosts for other Endpoints. It says nothing about this
            Endpoint&apos;s own reachability.
          </p>
        </Card>
      </div>
    </Section>
  )
}
