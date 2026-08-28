import type { ReactNode } from "react"

import { CodeValue, PageHeader, Section } from "../components/primitives"

export function SettingsScreen(): ReactNode {
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
              <button disabled type="button">
                Log out
              </button>
              <button className="button-secondary" disabled type="button">
                Revoke all
              </button>
            </div>
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
