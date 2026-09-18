import type { ReactNode } from "react"

import { Eyebrow } from "./ui"

export function Inspector({
  eyebrow,
  title,
  actions,
  children,
}: {
  readonly eyebrow: string
  readonly title: string
  readonly actions?: ReactNode
  readonly children: ReactNode
}): ReactNode {
  return (
    <aside aria-label={title} className="inspector">
      <header className="inspector__head">
        <Eyebrow>{eyebrow}</Eyebrow>
        <h2 className="inspector__title">{title}</h2>
      </header>
      <div className="inspector__body">{children}</div>
      {actions === undefined ? null : <footer className="inspector__foot">{actions}</footer>}
    </aside>
  )
}
