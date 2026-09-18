import type { ReactNode } from "react"

import { round } from "./geometry"
import { type Tone, toneColor } from "./tone"

export type ControlRound = {
  readonly atMs: number
  readonly peerCount: number
  readonly outcome: "succeeded" | "failed" | "empty"
}

const TONES: Record<ControlRound["outcome"], Tone> = {
  succeeded: "direct",
  failed: "failed",
  empty: "none",
}

/** Completed control rounds, oldest on the left. Never persisted, so restart clears it. */
export function RoundBars({ rounds }: { readonly rounds: readonly ControlRound[] }): ReactNode {
  const peak = Math.max(...rounds.map((entry) => entry.peerCount), 1)
  return (
    <figure className="rounds">
      <div className="rounds__bars">
        {rounds.map((entry) => (
          <span
            className="rounds__bar"
            key={entry.atMs}
            style={{
              background: toneColor(TONES[entry.outcome]),
              blockSize: `${round(Math.max((entry.peerCount / peak) * 100, 8))}%`,
            }}
          />
        ))}
      </div>
      <figcaption className="rounds__caption">
        peers synchronized per round
        <span className="visually-hidden">
          : {rounds.map((entry) => `${entry.outcome} with ${entry.peerCount} peers`).join(", ")}
        </span>
      </figcaption>
    </figure>
  )
}
