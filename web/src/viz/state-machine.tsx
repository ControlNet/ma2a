import type { ReactNode } from "react"

import { type Tone, toneColor } from "./tone"

export type ReachabilityState = {
  readonly name: string
  readonly gloss: string
  readonly tone: Tone
}

/**
 * The four named RelayReachability values, in the order the Runtime can reach
 * them. Not a score and not a progress bar: exactly one is current.
 */
export function StateMachine({
  states,
  current,
  compact = false,
}: {
  readonly states: readonly ReachabilityState[]
  readonly current: string
  readonly compact?: boolean
}): ReactNode {
  return (
    <ol className={compact ? "states states--compact" : "states"}>
      {states.map((state) => {
        const active = state.name === current
        return (
          <li
            aria-current={active ? "true" : undefined}
            className="state"
            key={state.name}
            style={active ? { borderColor: toneColor(state.tone) } : undefined}
          >
            <span
              aria-hidden="true"
              className="state__dot"
              style={{ background: active ? toneColor(state.tone) : "var(--track)" }}
            />
            <span
              className="state__name"
              style={active ? { color: toneColor(state.tone) } : undefined}
            >
              {state.name}
            </span>
            {compact || !active ? null : <span className="state__gloss">{state.gloss}</span>}
          </li>
        )
      })}
    </ol>
  )
}
