import type { ReactNode } from "react"

import type { Tone } from "../viz/tone"

export function Banner({
  tone,
  title,
  children,
  blocking = false,
}: {
  readonly tone: Tone
  readonly title: string
  readonly children: ReactNode
  readonly blocking?: boolean
}): ReactNode {
  return (
    <div className={`banner banner--${tone}`} role={blocking ? "alert" : "status"}>
      <span aria-hidden="true" className="banner__dot" />
      <div>
        <strong className="banner__title">{title}</strong>
        <p className="banner__body">{children}</p>
      </div>
    </div>
  )
}

export function EmptyState({
  title,
  children,
}: {
  readonly title: string
  readonly children: ReactNode
}): ReactNode {
  return (
    <div className="empty">
      <strong>{title}</strong>
      <p>{children}</p>
    </div>
  )
}

/**
 * Nothing is drawn from a fixture or from cached browser state, so the waiting
 * shape is deliberately empty rather than a plausible-looking skeleton of data.
 */
export function PendingSnapshot(): ReactNode {
  return (
    <div className="pending" role="status">
      <strong>Waiting for an authoritative snapshot</strong>
      <p>Lanes, rings and matrices appear only once the Runtime has answered.</p>
      <div aria-hidden="true" className="pending__bars">
        <span />
        <span />
        <span />
      </div>
    </div>
  )
}

export function SnapshotError({ onRetry }: { readonly onRetry: () => void }): ReactNode {
  return (
    <div className="snapshot-error" role="alert">
      <strong>Runtime snapshot is unavailable</strong>
      <p>The console could not load authoritative Runtime state.</p>
      <button className="button button--primary" onClick={onRetry} type="button">
        Retry
      </button>
    </div>
  )
}
