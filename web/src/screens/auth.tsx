import { type FormEvent, type ReactNode, useState } from "react"

import { ThemeToggle } from "../components/theme-toggle"
import { Pill } from "../components/ui"

function AuthFrame({ children }: { readonly children: ReactNode }): ReactNode {
  return (
    <main className="auth" id="main-content" tabIndex={-1}>
      <section className="auth__panel">
        <header className="auth__brand">
          <span aria-hidden="true" className="brand-mark">
            M2
          </span>
          <span className="auth__labels">
            <strong>MA2A</strong>
            <span className="eyebrow">Runtime Console · 127.0.0.1</span>
          </span>
          <ThemeToggle />
        </header>
        {children}
      </section>
    </main>
  )
}

export function LoginScreen({
  onLogin,
}: {
  readonly onLogin?: (password: string) => Promise<void>
}): ReactNode {
  const [pending, setPending] = useState(false)
  const [failed, setFailed] = useState(false)
  const submit = (event: FormEvent<HTMLFormElement>): void => {
    event.preventDefault()
    const password = new FormData(event.currentTarget).get("password")
    if (typeof password !== "string" || onLogin === undefined) return
    setPending(true)
    setFailed(false)
    void onLogin(password).then(
      () => undefined,
      () => {
        setPending(false)
        setFailed(true)
      },
    )
  }
  return (
    <AuthFrame>
      <div className="auth__copy">
        <h1>Sign in to this Runtime</h1>
      </div>
      <form className="form" onSubmit={submit}>
        <div className="field">
          <label className="field__label" htmlFor="password">
            Password
          </label>
          <div className="field__row">
            <input
              autoComplete="current-password"
              id="password"
              name="password"
              required
              type="password"
            />
            <button
              className="button button--primary"
              disabled={onLogin === undefined || pending}
              type="submit"
            >
              {pending ? "Signing in…" : "Sign in"}
            </button>
          </div>
        </div>
        {failed ? (
          <p className="field__help" role="alert">
            The password was not accepted.
          </p>
        ) : null}
      </form>
    </AuthFrame>
  )
}

const COMMANDS = [
  ["Set or reset the UI password", "ma2a ui init"],
  ["Start the WebUI", "ma2a ui start"],
] as const

export function SetupScreen(): ReactNode {
  return (
    <AuthFrame>
      <div className="auth__copy">
        <Pill filled tone="relay">
          Setup required
        </Pill>
        <h1>Create the first password from a trusted terminal</h1>
      </div>
      <dl className="commands">
        {COMMANDS.map(([label, command]) => (
          <div className="commands__row" key={command}>
            <dt>{label}</dt>
            <dd>
              <code className="command__line">{command}</code>
            </dd>
          </div>
        ))}
      </dl>
      <p className="field__help">Return here once the command succeeds.</p>
    </AuthFrame>
  )
}
