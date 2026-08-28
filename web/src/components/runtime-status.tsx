import type { ReactNode } from "react"

import type { RuntimeViewData } from "../view-model"
import { StatusText } from "./primitives"

function connectionTone(
  connection: RuntimeViewData["connection"],
): "success" | "warning" | "error" {
  switch (connection) {
    case "online":
      return "success"
    case "uncertain":
      return "warning"
    case "offline":
      return "error"
  }
}

export function RuntimeBanner({ runtime }: { readonly runtime: RuntimeViewData }): ReactNode {
  switch (runtime.connection) {
    case "online":
      return null
    case "uncertain":
      return (
        <div className="status-banner status-banner--warning" role="status">
          <strong>Snapshot is uncertain</strong>
          <span>Incremental updates are paused while a full snapshot is requested.</span>
        </div>
      )
    case "offline":
      return (
        <div className="status-banner status-banner--error" role="alert">
          <strong>Runtime is offline</strong>
          <span>Displayed values are not authoritative until the Runtime reconnects.</span>
        </div>
      )
  }
}

export function StateRail({ runtime }: { readonly runtime: RuntimeViewData }): ReactNode {
  return (
    <dl className="state-rail" aria-label="Runtime status summary">
      <div>
        <dt>Runtime</dt>
        <dd>
          <StatusText tone={connectionTone(runtime.connection)}>{runtime.connection}</StatusText>
        </dd>
      </div>
      <div>
        <dt>Revision</dt>
        <dd className="mono-value">{runtime.revision}</dd>
      </div>
      <div>
        <dt>Spaces</dt>
        <dd>{runtime.spaces.length}</dd>
      </div>
      <div>
        <dt>Observed path</dt>
        <dd>{runtime.endpoint.observedPath}</dd>
      </div>
    </dl>
  )
}

export function PendingRuntime(): ReactNode {
  return (
    <div className="pending-runtime" role="status">
      <strong>Waiting for an authoritative snapshot</strong>
      <span>The shell never substitutes test fixtures or cached browser state.</span>
      <div aria-hidden="true" className="skeleton-lines">
        <span />
        <span />
        <span />
      </div>
    </div>
  )
}
