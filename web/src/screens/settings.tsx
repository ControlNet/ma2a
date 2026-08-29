import { type ReactNode, useState } from "react"

import { CodeValue, PageHeader, Section } from "../components/primitives"

export function SettingsScreen({
  onLogout,
}: {
  readonly onLogout?: (() => Promise<void>) | undefined
}): ReactNode {
  const [logoutPending, setLogoutPending] = useState(false)
  const [logoutFailed, setLogoutFailed] = useState(false)
  const logout = (): void => {
    if (onLogout === undefined) {
      return
    }
    setLogoutPending(true)
    setLogoutFailed(false)
    void onLogout().then(
      () => undefined,
      () => {
        setLogoutPending(false)
        setLogoutFailed(true)
      },
    )
  }

  return (
    <div className="page-stack">
      <PageHeader
        description="Browser sessions are revocable; password creation and reset stay on trusted local IPC."
        title="Settings"
      />
      <div className="settings-sections">
        <Section title="Session">
          <div className="settings-row">
            <div>
              <strong>Current browser session</strong>
              <p>Authentication uses a host-only, HttpOnly, SameSite=Strict cookie.</p>
            </div>
            <div className="action-cluster">
              <button
                disabled={onLogout === undefined || logoutPending}
                onClick={logout}
                type="button"
              >
                {logoutPending ? "Logging out..." : "Log out"}
              </button>
              <button className="button-secondary" disabled type="button">
                Revoke all
              </button>
            </div>
            {logoutFailed ? <p role="alert">The current session could not be logged out.</p> : null}
          </div>
        </Section>
        <Section title="Password recovery">
          <div className="settings-row">
            <div>
              <strong>Reset from the local terminal</strong>
              <p>A password reset revokes all existing Web sessions.</p>
            </div>
            <CodeValue>ma2a ui password set</CodeValue>
          </div>
        </Section>
        <Section title="Browser storage">
          <p className="content-measure">
            MA2A does not place passphrases, bearer tokens, verifiers, invite secrets, or private
            key material in localStorage, sessionStorage, or URLs.
          </p>
        </Section>
      </div>
    </div>
  )
}
