import type { FormEvent, ReactNode } from "react"

export function Field({
  id,
  label,
  help,
  children,
}: {
  readonly id: string
  readonly label: string
  readonly help?: string
  readonly children: ReactNode
}): ReactNode {
  return (
    <div className="field">
      <label className="field__label" htmlFor={id}>
        {label}
      </label>
      {children}
      {help === undefined ? null : <p className="field__help">{help}</p>}
    </div>
  )
}

export function Form({
  label,
  onSubmit,
  submitLabel,
  disabled = false,
  danger = false,
  status,
  children,
}: {
  readonly label: string
  readonly onSubmit: (data: FormData) => void
  readonly submitLabel: string
  readonly disabled?: boolean
  readonly danger?: boolean
  readonly status?: string
  readonly children: ReactNode
}): ReactNode {
  const submit = (event: FormEvent<HTMLFormElement>): void => {
    event.preventDefault()
    onSubmit(new FormData(event.currentTarget))
  }
  return (
    <form aria-label={label} className="form" onSubmit={submit}>
      {children}
      <button
        className={danger ? "button button--danger" : "button button--primary"}
        disabled={disabled}
        type="submit"
      >
        {submitLabel}
      </button>
      {status === undefined ? null : (
        <p className="field__help" role="status">
          {status}
        </p>
      )}
    </form>
  )
}

export function text(data: FormData, key: string): string {
  const value = data.get(key)
  return typeof value === "string" ? value : ""
}
