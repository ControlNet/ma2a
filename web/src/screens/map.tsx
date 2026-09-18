import type { ReactNode } from "react"
import { PendingSnapshot } from "../components/feedback"
import { Inspector } from "../components/inspector"
import { Button, Card, Pill, Section } from "../components/ui"
import type { RuntimeActions } from "../runtime-actions"
import type { RuntimeViewData } from "../view-model"
import { Donut } from "../viz/donut"
import { LaneMap } from "../viz/lane-map"
import { MeterList } from "../viz/meter-list"
import { RoundBars } from "../viz/round-bars"
import { StackBar } from "../viz/stack-bar"
import { StateMachine } from "../viz/state-machine"
import {
  boundMeters,
  echoSegments,
  laneList,
  pathParts,
  peerList,
  REACHABILITY_STATES,
} from "./derive"

export function MapScreen({
  runtime,
  selected,
  onSelect,
}: {
  readonly runtime: RuntimeViewData | undefined
  readonly selected: string | undefined
  readonly onSelect: (id: string) => void
}): ReactNode {
  if (runtime === undefined) return <PendingSnapshot />
  return (
    <Section
      description="One Endpoint identity, one lane per independently authorized Space, and whatever Iroh currently reports about reaching anyone."
      title="Map"
    >
      <LaneMap
        endpointId={runtime.endpoint.id}
        lanes={laneList(runtime)}
        onSelect={onSelect}
        peers={peerList(runtime)}
        {...(selected === undefined ? {} : { selected })}
      />
      <ul className="lane-legend">
        <li>
          <span aria-hidden="true" className="lane-legend__swatch lane-legend__swatch--line" />
          lane: signed membership
        </li>
        <li style={{ color: "var(--observed-direct)" }}>
          <span aria-hidden="true" className="lane-legend__swatch" />
          direct path observed
        </li>
        <li style={{ color: "var(--observed-relay)" }}>
          <span aria-hidden="true" className="lane-legend__swatch" />
          relay path observed
        </li>
        <li style={{ color: "var(--observed-none)" }}>
          <span aria-hidden="true" className="lane-legend__swatch lane-legend__swatch--dashed" />
          no observation retained
        </li>
      </ul>
    </Section>
  )
}

export function MapInspector({
  runtime,
  actions,
  onCreateSpace,
}: {
  readonly runtime: RuntimeViewData | undefined
  readonly actions: RuntimeActions | undefined
  readonly onCreateSpace: () => void
}): ReactNode {
  if (runtime === undefined) {
    return (
      <Inspector eyebrow="Runtime" title="No snapshot">
        <p className="field__help">Nothing is drawn until the Runtime answers.</p>
      </Inspector>
    )
  }
  const echo = echoSegments(runtime)
  return (
    <Inspector
      actions={
        <Button disabled={actions === undefined} onClick={onCreateSpace} variant="primary">
          Create Space
        </Button>
      }
      eyebrow="Runtime"
      title="Nothing selected"
    >
      <Card label="Relay reachability">
        <StateMachine compact current={runtime.reachability.state} states={REACHABILITY_STATES} />
        <p className="field__help">{runtime.reachability.detail}</p>
      </Card>
      <Card label="Observed paths">
        <StackBar caption="Observed paths by peer" parts={pathParts(runtime)} />
      </Card>
      <Card label="Echo">
        <div className="cluster">
          <Donut
            label="echo"
            segments={echo.segments}
            value={echo.total === 0 ? "0" : String(echo.total)}
          />
          <div className="stack-4">
            <Pill tone="direct">{runtime.echoTotals.successes} ok</Pill>
            <Pill tone="failed">{runtime.echoTotals.failures} failed</Pill>
          </div>
        </div>
        <p className="field__help">Bounded totals only. No payload or target is retained.</p>
      </Card>
      <Card label="Control rounds">
        {runtime.controlRounds.length === 0 ? (
          <p className="field__help">No round has completed since this Runtime started.</p>
        ) : (
          <RoundBars rounds={runtime.controlRounds} />
        )}
        <p className="field__help">
          Deterministic 60 to 89 second interval, at most four peers per Space per round. Retained
          in memory only.
        </p>
      </Card>
      <Card label="Bounded capacity">
        <MeterList label="Bounded capacity" meters={boundMeters(runtime)} />
      </Card>
    </Inspector>
  )
}
