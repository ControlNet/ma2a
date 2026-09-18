export function clamp(value: number, min: number, max: number): number {
  return Math.min(Math.max(value, min), max)
}

export function round(value: number): number {
  return Math.round(value * 10) / 10
}

export function circumference(radius: number): number {
  return 2 * Math.PI * radius
}

/** `stroke-dasharray` that fills `fraction` of a circle drawn from twelve o'clock. */
export function ringDash(fraction: number, radius: number): string {
  const total = circumference(radius)
  return `${round(total * clamp(fraction, 0, 1))} ${round(total)}`
}

export type Point = readonly [number, number]

export function extent(values: readonly number[]): readonly [number, number] {
  const low = Math.min(...values)
  const high = Math.max(...values)
  return high === low ? [low - 1, high + 1] : [low, high]
}

/** Evenly spaces `values` across the box and scales them to its height. */
export function plot(values: readonly number[], width: number, height: number, pad = 4): Point[] {
  const [low, high] = extent(values)
  const step = values.length > 1 ? (width - pad * 2) / (values.length - 1) : 0
  return values.map((value, index) => [
    round(pad + index * step),
    round(height - pad - ((value - low) / (high - low)) * (height - pad * 2)),
  ])
}

export function polyline(points: readonly Point[]): string {
  return points.map(([x, y]) => `${x},${y}`).join(" ")
}

export function area(points: readonly Point[], height: number): string {
  const first = points.at(0)
  const last = points.at(-1)
  if (first === undefined || last === undefined) return ""
  return `${first[0]},${height} ${polyline(points)} ${last[0]},${height}`
}

/** Fractional position along a 270-degree dial that opens at the bottom. */
export function dialPoint(fraction: number, cx: number, cy: number, radius: number): Point {
  const angle = ((135 + clamp(fraction, 0, 1) * 270) * Math.PI) / 180
  return [round(cx + radius * Math.cos(angle)), round(cy + radius * Math.sin(angle))]
}

export function dialArc(from: number, to: number, cx: number, cy: number, radius: number): string {
  const [x0, y0] = dialPoint(from, cx, cy, radius)
  const [x1, y1] = dialPoint(to, cx, cy, radius)
  const sweep = to - from > 0.5 ? 1 : 0
  return `M${x0} ${y0} A${radius} ${radius} 0 ${sweep} 1 ${x1} ${y1}`
}
