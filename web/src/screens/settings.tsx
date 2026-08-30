import { type ReactNode, useState } from "react"

import { CodeValue, PageHeader, Section } from "../components/primitives"
import type { RuntimeActions } from "../runtime-actions"
import type { RuntimeViewData } from "../view-model"

export function SettingsScreen({
  onLogout,
  actions,
  runtime,
}: {
  readonly onLogout?: (() => Promise<void>) | undefined
  readonly actions?: RuntimeActions | undefined
  readonly runtime?: RuntimeViewData | undefined
}): ReactNode {
  const [logoutPending, setLogoutPending] = useState(false)
  const [logoutFailed, setLogoutFailed] = useState(false)
  const [revokeMessage, setRevokeMessage] = useState<string | undefined>()
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
  const revokeAll = (): void => {
    if (actions === undefined) return
    setRevokeMessage("Revoking sessions...")
    void actions.revokeSessions().then(
      () => {
        setRevokeMessage("All browser sessions were revoked. Sign in again.")
        void onLogout?.()
      },
      () => setRevokeMessage("Sessions could not be revoked."),
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
              <p>
                Authentication uses a host-only, HttpOnly, SameSite=Strict cookie. Active sessions:{" "}
                {runtime?.uiAuth.active_sessions ?? "unknown"}.
              </p>
            </div>
            <div className="action-cluster">
              <button
                disabled={onLogout === undefined || logoutPending}
                onClick={logout}
                type="button"
              >
                {logoutPending ? "Logging out..." : "Log out"}
              </button>
              <button
                className="button-danger"
                disabled={actions === undefined}
                onClick={revokeAll}
                type="button"
              >
                Revoke all
              </button>
            </div>
            {logoutFailed ? <p role="alert">The current session could not be logged out.</p> : null}
            {revokeMessage === undefined ? null : <p role="status">{revokeMessage}</p>}
          </div>
        </Section>
        <Section title="Password recovery">
          <div className="settings-row">
            <div>
              <strong>Reset from the local terminal</strong>
              <p>A password reset revokes all existing Web sessions.</p>
            </div>
            <CodeValue>ma2a ui password reset</CodeValue>
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
