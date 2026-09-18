import type { ReactNode } from "react"

import { round } from "./geometry"
import { type Tone, toneColor } from "./tone"

export type TrackObservation = {
  readonly atMs: number
  readonly tone: Tone
  readonly label: string
  readonly rttMs: number | undefined
}

const WIDTH = 560
const HEIGHT = 74

/**
 * The bounded observation buffer on a real time axis. A relay-to-direct upgrade
 * reads as a colour change in place, and the newest observation sits at "now".
 */
export function ObservationTrack({
  observations,
}: {
  readonly observations: readonly TrackObservation[]
}): ReactNode {
  const first = observations.at(0)
  const last = observations.at(-1)
  if (first === undefined || last === undefined) return null
  const span = Math.max(last.atMs - first.atMs, 1)
  const x = (atMs: number): number => round(14 + ((atMs - first.atMs) / span) * (WIDTH - 40))
  const summary = observations
    .map((item) => `${item.label}${item.rttMs === undefined ? "" : ` ${item.rttMs} ms`}`)
    .join(", ")
  return (
    <svg
      aria-label={`Retained observations, oldest first: ${summary}`}
      className="chart"
      height={HEIGHT}
      role="img"
      viewBox={`0 0 ${WIDTH} ${HEIGHT}`}
      width="100%"
    >
      <line stroke="var(--line)" x1="14" x2={WIDTH - 26} y1="30" y2="30" />
      {observations.map((item) => (
        <g key={`${item.atMs}-${item.label}`}>
          <circle cx={x(item.atMs)} cy="30" fill={toneColor(item.tone)} r="5" />
          {item.rttMs === undefined ? null : (
            <text className="chart__caption" textAnchor="middle" x={x(item.atMs)} y="52">
              {item.rttMs}
            </text>
          )}
        </g>
      ))}
      <text className="chart__caption" x="0" y="14">
        oldest
      </text>
      <text className="chart__caption chart__caption--accent" textAnchor="end" x={WIDTH} y="14">
        newest
      </text>
      <text className="chart__caption" x="0" y={HEIGHT - 4}>
        {observations.length} of 8 retained
      </text>
    </svg>
  )
}
