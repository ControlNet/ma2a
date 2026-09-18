import type { ReactNode } from "react"

import { circumference, round } from "./geometry"
import { type Tone, toneColor } from "./tone"

export type DonutSegment = {
  readonly tone: Tone
  readonly value: number
  readonly label: string
}

export function Donut({
  segments,
  value,
  label,
  size = 88,
}: {
  readonly segments: readonly DonutSegment[]
  readonly value: string
  readonly label: string
  readonly size?: number
}): ReactNode {
  const stroke = Math.round(size / 8)
  const radius = (size - stroke) / 2
  const centre = size / 2
  const total = segments.reduce((sum, segment) => sum + segment.value, 0) || 1
  const length = circumference(radius)
  let offset = 0
  return (
    <svg
      aria-label={`${label}: ${segments.map((s) => `${s.value} ${s.label}`).join(", ")}`}
      className="chart"
      height={size}
      role="img"
      viewBox={`0 0 ${size} ${size}`}
      width={size}
    >
      <circle
        cx={centre}
        cy={centre}
        fill="none"
        r={radius}
        stroke="var(--track)"
        strokeWidth={stroke}
      />
      {segments.map((segment) => {
        const fraction = segment.value / total
        const dash = `${round(Math.max(length * fraction - 2, 0))} ${round(length)}`
        const element = (
          <circle
            cx={centre}
            cy={centre}
            fill="none"
            key={segment.label}
            r={radius}
            stroke={toneColor(segment.tone)}
            strokeDasharray={dash}
            strokeDashoffset={round(-length * offset)}
            strokeWidth={stroke}
            transform={`rotate(-90 ${centre} ${centre})`}
          />
        )
        offset += fraction
        return element
      })}
      <text className="chart__value" textAnchor="middle" x={centre} y={centre + 2}>
        {value}
      </text>
      <text className="chart__caption" textAnchor="middle" x={centre} y={centre + 16}>
        {label}
      </text>
    </svg>
  )
}
