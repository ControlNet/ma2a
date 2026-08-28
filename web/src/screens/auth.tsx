import type { FormEvent, ReactNode } from "react"

import { CodeValue } from "../components/primitives"

function AuthFrame({ children }: { readonly children: ReactNode }): ReactNode {
  return (
    <main className="auth-frame" id="main-content" tabIndex={-1}>
      <section className="auth-panel">
        <header className="auth-brand">
          <span aria-hidden="true" className="brand-mark">
            M2
          </span>
          <div>
            <strong>MA2A</strong>
            <span>Runtime Console</span>
          </div>
        </header>
        {children}
      </section>
    </main>
  )
}

export function LoginScreen({
  onLogin,
}: {
  readonly onLogin?: (passphrase: string) => void
}): ReactNode {
  const submit = (event: FormEvent<HTMLFormElement>): void => {
    event.preventDefault()
    const form = new FormData(event.currentTarget)
    const passphrase = form.get("passphrase")
    if (typeof passphrase === "string") {
      onLogin?.(passphrase)
    }
  }

  return (
    <AuthFrame>
      <div className="auth-copy">
        <h1>Sign in to this Runtime</h1>
        <p>
          Your passphrase is sent only to the same-origin Runtime and is never stored by the
          browser.
        </p>
      </div>
      <form className="form-stack" onSubmit={submit}>
        <label htmlFor="passphrase">Passphrase</label>
        <input
          autoComplete="current-password"
          id="passphrase"
          name="passphrase"
          required
          type="password"
        />
        <p className="field-help">Sessions use a host-only, HttpOnly, SameSite=Strict cookie.</p>
        <button disabled={onLogin === undefined} type="submit">
          Sign in
        </button>
      </form>
    </AuthFrame>
  )
}

export function SetupScreen(): ReactNode {
  return (
    <AuthFrame>
      <div className="auth-copy">
        <p className="notice-label">Setup required</p>
        <h1>Create the first passphrase from a trusted terminal</h1>
        <p>
          Management state remains hidden until the local Runtime has a password verifier. Password
          creation and reset are intentionally unavailable in the browser.
        </p>
      </div>
      <section className="command-block" aria-labelledby="setup-commands-title">
        <h2 id="setup-commands-title">Trusted local commands</h2>
        <div>
          <span>Initialize a new Runtime</span>
          <CodeValue>ma2a init</CodeValue>
        </div>
        <div>
          <span>Set or reset the UI passphrase</span>
          <CodeValue>ma2a ui password set</CodeValue>
        </div>
      </section>
      <p className="field-help">Return to this page after the trusted local command succeeds.</p>
    </AuthFrame>
  )
}
