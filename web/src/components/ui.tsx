import type { ReactNode } from "react"

import type { Tone } from "../viz/tone"

export function Eyebrow({ children }: { readonly children: ReactNode }): ReactNode {
  return <span className="eyebrow">{children}</span>
}

export function Mono({ children }: { readonly children: ReactNode }): ReactNode {
  return <span className="mono">{children}</span>
}

/** Status is never colour alone: the pill always carries its own word. */
export function Pill({
  tone,
  filled = false,
  children,
}: {
  readonly tone: Tone
  readonly filled?: boolean
  readonly children: ReactNode
}): ReactNode {
  return (
    <span className={`pill pill--${tone}${filled ? " pill--filled" : ""}`}>
      <span aria-hidden="true" className="pill__dot" />
      {children}
    </span>
  )
}

export function Card({
  label,
  actions,
  children,
}: {
  readonly label?: string
  readonly actions?: ReactNode
  readonly children: ReactNode
}): ReactNode {
  return (
    <section className="card">
      {label === undefined && actions === undefined ? null : (
        <header className="card__head">
          {label === undefined ? null : <Eyebrow>{label}</Eyebrow>}
          {actions === undefined ? null : <div className="cluster">{actions}</div>}
        </header>
      )}
      <div className="card__body">{children}</div>
    </section>
  )
}

export function Section({
  title,
  actions,
  children,
}: {
  readonly title: string
  readonly actions?: ReactNode
  readonly children: ReactNode
}): ReactNode {
  return (
    <section className="section">
      <header className="section__head">
        <h1 className="section__title">{title}</h1>
        {actions === undefined ? null : <div className="cluster">{actions}</div>}
      </header>
      {children}
    </section>
  )
}

export type ButtonVariant = "primary" | "ghost" | "danger"

export function Button({
  variant = "ghost",
  disabled = false,
  onClick,
  children,
}: {
  readonly variant?: ButtonVariant
  readonly disabled?: boolean
  readonly onClick?: () => void
  readonly children: ReactNode
}): ReactNode {
  return (
    <button
      className={`button button--${variant}`}
      disabled={disabled}
      onClick={onClick}
      type="button"
    >
      {children}
    </button>
  )
}

/** A long identifier, wrapped rather than shrunk, with a copy affordance. */
export function Identifier({
  value,
  onCopy,
  copied = false,
}: {
  readonly value: string
  readonly onCopy?: () => void
  readonly copied?: boolean
}): ReactNode {
  return (
    <span className="identifier">
      <code className="identifier__value">{value}</code>
      {onCopy === undefined ? null : (
        <button
          aria-label={`Copy full identifier ${value}`}
          className="button button--ghost button--compact"
          onClick={onCopy}
          type="button"
        >
          {copied ? "Copied" : "Copy"}
        </button>
      )}
    </span>
  )
}
