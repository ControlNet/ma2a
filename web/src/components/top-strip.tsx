import type { ReactNode } from "react"

import type { RuntimeViewData } from "../view-model"
import { ThemeToggle } from "./theme-toggle"
import { Pill } from "./ui"

function connectionPill(connection: RuntimeViewData["connection"]): ReactNode {
  if (connection === "online") return <Pill tone="direct">snapshot live</Pill>
  if (connection === "uncertain") return <Pill tone="relay">snapshot uncertain</Pill>
  return <Pill tone="failed">runtime offline</Pill>
}

export function TopStrip({
  runtime,
}: {
  readonly runtime: RuntimeViewData | undefined
}): ReactNode {
  return (
    <header className="top-strip">
      <div className="top-strip__identity">
        <span aria-hidden="true" className="brand-mark">
          M2
        </span>
        <span className="top-strip__labels">
          <span className="eyebrow">This Endpoint</span>
          <span className="mono">{runtime === undefined ? "waiting" : runtime.endpoint.id}</span>
        </span>
      </div>
      <div className="top-strip__state">
        {runtime === undefined ? (
          <Pill tone="none">no snapshot</Pill>
        ) : (
          <>
            <Pill filled tone={runtime.reachability.tone}>
              {runtime.reachability.state}
            </Pill>
            <span className="top-strip__revision">
              <span className="eyebrow">rev</span>
              <span className="mono">{runtime.revision}</span>
            </span>
            {connectionPill(runtime.connection)}
          </>
        )}
        <ThemeToggle />
      </div>
    </header>
  )
}
