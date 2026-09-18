/**
 * The two colour groups from tokens.css, kept apart on purpose.
 * `accent` says "you can act on this". Every other tone says "Iroh observed
 * this". A signed authorization fact is never painted with an observed tone.
 */
export type Tone = "accent" | "direct" | "relay" | "failed" | "none"

const COLOR: Record<Tone, string> = {
  accent: "var(--accent)",
  direct: "var(--observed-direct)",
  relay: "var(--observed-relay)",
  failed: "var(--observed-failed)",
  none: "var(--observed-none)",
}

const SOFT: Record<Tone, string> = {
  accent: "var(--accent-soft)",
  direct: "var(--observed-direct-soft)",
  relay: "var(--observed-relay-soft)",
  failed: "var(--observed-failed-soft)",
  none: "var(--observed-none-soft)",
}

export function toneColor(tone: Tone): string {
  return COLOR[tone]
}

export function toneSoft(tone: Tone): string {
  return SOFT[tone]
}

/** An unobserved path is drawn dashed so it never reads as a failure. */
export function toneDash(tone: Tone): string | undefined {
  return tone === "none" ? "4 3" : undefined
}
