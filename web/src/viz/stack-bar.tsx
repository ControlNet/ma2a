import type { ReactNode } from "react"

import { round } from "./geometry"
import { type Tone, toneColor } from "./tone"

export type StackPart = {
  readonly label: string
  readonly value: number
  readonly tone: Tone
}

export function StackBar({
  parts,
  caption,
}: {
  readonly parts: readonly StackPart[]
  readonly caption: string
}): ReactNode {
  const total = parts.reduce((sum, part) => sum + part.value, 0) || 1
  const present = parts.filter((part) => part.value > 0)
  return (
    <figure className="stack">
      <div aria-hidden="true" className="stack__bar">
        {present.map((part) => (
          <span
            className="stack__segment"
            key={part.label}
            style={{
              background: toneColor(part.tone),
              inlineSize: `${round((part.value / total) * 100)}%`,
            }}
          />
        ))}
      </div>
      <figcaption className="stack__legend">
        <span className="visually-hidden">{caption}</span>
        {parts.map((part) => (
          <span className="stack__key" key={part.label}>
            <span
              aria-hidden="true"
              className="stack__dot"
              style={{ background: toneColor(part.tone) }}
            />
            {part.label} {part.value}
          </span>
        ))}
      </figcaption>
    </figure>
  )
}
