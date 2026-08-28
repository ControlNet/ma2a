import type { ReactNode } from "react"

export function PageHeader({
  title,
  description,
  children,
}: {
  readonly title: string
  readonly description: string
  readonly children?: ReactNode
}): ReactNode {
  return (
    <header className="page-header">
      <div>
        <h1>{title}</h1>
        <p>{description}</p>
      </div>
      {children === undefined ? null : <div className="action-cluster">{children}</div>}
    </header>
  )
}

export function Section({
  title,
  description,
  children,
}: {
  readonly title: string
  readonly description?: string
  readonly children: ReactNode
}): ReactNode {
  return (
    <section className="section-stack">
      <div className="section-heading">
        <h2>{title}</h2>
        {description === undefined ? null : <p>{description}</p>}
      </div>
      {children}
    </section>
  )
}

export function EmptyState({
  title,
  description,
}: {
  readonly title: string
  readonly description: string
}): ReactNode {
  return (
    <div className="empty-state">
      <strong>{title}</strong>
      <p>{description}</p>
    </div>
  )
}

export function StatusText({
  tone,
  children,
}: {
  readonly tone: "neutral" | "success" | "warning" | "error"
  readonly children: ReactNode
}): ReactNode {
  return (
    <span className={`status-text status-text--${tone}`}>
      <span aria-hidden="true" className="status-dot" />
      {children}
    </span>
  )
}

export function CodeValue({ children }: { readonly children: ReactNode }): ReactNode {
  return <code className="code-value">{children}</code>
}
