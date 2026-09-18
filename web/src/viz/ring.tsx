import type { ReactNode } from "react"

import { circumference, clamp, ringDash } from "./geometry"
import { type Tone, toneColor } from "./tone"

export function Ring({
  fraction,
  value,
  label,
  tone = "accent",
  size = 88,
}: {
  readonly fraction: number
  readonly value: string
  readonly label: string
  readonly tone?: Tone
  readonly size?: number
}): ReactNode {
  const stroke = Math.round(size / 12)
  const radius = (size - stroke) / 2
  const centre = size / 2
  return (
    <svg
      aria-label={`${label}: ${value}`}
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
      <circle
        cx={centre}
        cy={centre}
        fill="none"
        r={radius}
        stroke={toneColor(tone)}
        strokeDasharray={ringDash(fraction, radius)}
        strokeLinecap="round"
        strokeWidth={stroke}
        transform={`rotate(-90 ${centre} ${centre})`}
      />
      <text className="chart__value" textAnchor="middle" x={centre} y={centre + 2}>
        {value}
      </text>
      <text className="chart__caption" textAnchor="middle" x={centre} y={centre + 16}>
        {label}
      </text>
      <desc>{`${clamp(Math.round(fraction * 100), 0, 100)} percent`}</desc>
      <title>{circumference(radius) > 0 ? `${label} ${value}` : label}</title>
    </svg>
  )
}
