import type { ReactNode } from "react"

import { clamp, round } from "./geometry"
import { type Tone, toneColor } from "./tone"

export type Meter = {
  readonly label: string
  readonly fraction: number
  readonly value: string
  readonly tone: Tone
  readonly note?: string
}

/**
 * One row per documented bound or countdown. The number is always spelled out,
 * so the bar is a second reading of the same fact rather than the only one.
 */
export function MeterList({
  meters,
  label,
}: {
  readonly meters: readonly Meter[]
  readonly label: string
}): ReactNode {
  return (
    <ul aria-label={label} className="meters">
      {meters.map((meter) => (
        <li className="meter" key={meter.label}>
          <span className="meter__label">{meter.label}</span>
          <span className="meter__value">{meter.value}</span>
          <span aria-hidden="true" className="meter__track">
            <span
              className="meter__fill"
              style={{
                background: toneColor(meter.tone),
                inlineSize: `${round(clamp(meter.fraction, 0, 1) * 100)}%`,
              }}
            />
          </span>
          {meter.note === undefined ? null : <span className="meter__note">{meter.note}</span>}
        </li>
      ))}
    </ul>
  )
}
