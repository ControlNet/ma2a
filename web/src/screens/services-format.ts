import type { ServiceExposure } from "../api/services"

const UNITS = ["B", "KiB", "MiB", "GiB", "TiB"] as const

/** Byte counts in binary units, exact below 1 KiB. */
export function byteCount(bytes: number): string {
  let value = bytes
  let unit = 0
  while (value >= 1024 && unit < UNITS.length - 1) {
    value /= 1024
    unit += 1
  }
  return unit === 0 ? `${bytes} B` : `${value.toFixed(1)} ${UNITS[unit]}`
}

export function duration(milliseconds: number): string {
  if (milliseconds < 1_000) return `${milliseconds} ms`
  const seconds = Math.floor(milliseconds / 1_000)
  if (seconds < 60) return `${(milliseconds / 1_000).toFixed(1)} s`
  const minutes = Math.floor(seconds / 60)
  if (minutes < 60) return `${minutes} m ${seconds % 60} s`
  return `${Math.floor(minutes / 60)} h ${minutes % 60} m`
}

/** What one exposure lets a granted member reach, in the CLI's own terms. */
export function exposureScope(exposure: ServiceExposure): string {
  switch (exposure.kind) {
    case "files_root": {
      const access = exposure.access === "read_only" ? "read-only" : "read-write"
      return `${exposure.root ?? "?"}:/ → ${exposure.path ?? "?"} (${access})`
    }
    case "proxy": {
      const ports =
        exposure.port_start === exposure.port_end
          ? `${exposure.port_start}`
          : `${exposure.port_start}-${exposure.port_end}`
      return `${exposure.host ?? "?"} port ${ports}`
    }
    case "exec":
      return "one-shot programs"
    case "shell":
      return "interactive terminals"
    case "process":
      return "long-running Processes"
    case "agent":
      return `Agent ${exposure.agent ?? "?"}`
    case "service":
      return `service ${exposure.service ?? "?"}`
  }
}
