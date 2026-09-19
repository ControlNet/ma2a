import { type ReactNode, useState } from "react"

import { Inspector } from "../components/inspector"
import { ThemeToggle } from "../components/theme-toggle"
import { Button, Card, Pill, Section } from "../components/ui"
import type { RuntimeActions } from "../runtime-actions"
import type { RuntimeViewData } from "../view-model"
import { type Boundary, TrustChain } from "../viz/trust-chain"

const SECRETS = ["Passphrase", "Invitation ticket secret", "Endpoint and Space private keys"]

/**
 * Rows follow SECRETS. Retention is the question, not handling: a passphrase does
 * pass through the browser on sign-in, and protected storage keeps only an
 * Argon2id verifier, never the passphrase itself.
 */
const BOUNDARIES: readonly Boundary[] = [
  {
    name: "Browser",
    guard: "host-only HttpOnly cookie",
    presence: ["transient", "never", "never"],
  },
  {
    name: "Loopback HTTP",
    guard: "same-origin, Host pinned",
    presence: ["transient", "never", "never"],
  },
  {
    name: "Private IPC",
    guard: "current-user UID or SID",
    presence: ["transient", "transient", "never"],
  },
  {
    name: "Runtime memory",
    guard: "zeroized after use",
    presence: ["transient", "transient", "transient"],
  },
  {
    name: "Protected storage",
    guard: "verifier and digests only",
    presence: ["never", "never", "retained"],
  },
  {
    name: "Owner-only file",
    guard: "written once on request",
    presence: ["never", "retained", "never"],
  },
]

export function SettingsScreen({
  runtime,
}: {
  readonly runtime: RuntimeViewData | undefined
}): ReactNode {
  return (
    <Section
      description="The browser is the least trusted surface in this system. This page shows exactly how little it is allowed to hold."
      title="Settings"
    >
      <Card label="Where each secret may be persisted or retained">
        <div className="scroll-x">
          <TrustChain boundaries={BOUNDARIES} secrets={SECRETS} />
        </div>
        <ul className="trust-legend">
          <li>
            <span aria-hidden="true" className="trust__mark trust__mark--retained" />
            retained here
          </li>
          <li>
            <span aria-hidden="true" className="trust__mark trust__mark--transient" />
            handled in passing, not kept
          </li>
          <li>
            <span aria-hidden="true" className="trust__mark trust__mark--never" />
            never present
          </li>
        </ul>
        <p className="field__help">
          Handling is not retention. Signing in necessarily puts the passphrase in browser memory
          and sends it over same-origin loopback; nothing keeps it. Protected storage holds an
          Argon2id verifier and an invitation digest, never the passphrase or the ticket secret
          themselves. The ticket file is the one place a ticket secret is retained, written once on
          request for the operator to transfer and delete.
        </p>
      </Card>
      <div className="split">
        <Card label="Appearance">
          <ThemeToggle />
          <p className="field__help">
            Follows the operating system until you pick one. Only this display preference is stored
            in the browser.
          </p>
        </Card>
        <Card label="Password recovery">
          <p className="field__help">
            A reset increments the authentication epoch and revokes every prior session. There is no
            browser route for creating or resetting a passphrase.
          </p>
          <div className="command">
            <code className="command__line">ma2a ui password reset</code>
            <code className="command__line">ma2a ui sessions revoke-all</code>
          </div>
        </Card>
        <Card label="Browser storage">
          <p className="field__help">
            MA2A places no passphrase, bearer token, verifier, invite secret or private key material
            in localStorage, sessionStorage or a URL.
          </p>
          <div className="cluster">
            <Pill tone="accent">{runtime?.uiAuth.active_sessions ?? "unknown"} active session</Pill>
          </div>
        </Card>
      </div>
    </Section>
  )
}

export function SettingsInspector({
  runtime,
  actions,
  onLogout,
  onSessionsRevoked,
}: {
  readonly runtime: RuntimeViewData | undefined
  readonly actions: RuntimeActions | undefined
  readonly onLogout: (() => Promise<void>) | undefined
  readonly onSessionsRevoked: (() => void) | undefined
}): ReactNode {
  const [message, setMessage] = useState<string | undefined>()
  const [pending, setPending] = useState(false)
  const logout = (): void => {
    if (onLogout === undefined) return
    setPending(true)
    void onLogout().then(
      () => undefined,
      () => {
        setPending(false)
        setMessage("The current session could not be logged out.")
      },
    )
  }
  const revokeAll = (): void => {
    if (actions === undefined) return
    setMessage("Revoking sessions…")
    void actions.revokeSessions().then(
      () => {
        setMessage("All browser sessions were revoked. Sign in again.")
        onSessionsRevoked?.()
      },
      () => setMessage("Sessions could not be revoked."),
    )
  }
  return (
    <Inspector eyebrow="Session" title="This browser session">
      <Card label="Cookie boundary">
        <div className="cluster">
          <Pill tone="accent">host-only</Pill>
          <Pill tone="accent">HttpOnly</Pill>
          <Pill tone="accent">SameSite=Strict</Pill>
          <Pill tone="accent">CSRF per session</Pill>
        </div>
        <p className="field__help">
          Active sessions: {runtime?.uiAuth.active_sessions ?? "unknown"}. An expired or revoked
          session closes the event stream without emitting any state-bearing frame.
        </p>
      </Card>
      <div className="cluster">
        <Button disabled={onLogout === undefined || pending} onClick={logout}>
          {pending ? "Logging out…" : "Log out"}
        </Button>
        <Button disabled={actions === undefined} onClick={revokeAll} variant="danger">
          Revoke all sessions
        </Button>
      </div>
      {message === undefined ? null : (
        <p className="field__help" role="status">
          {message}
        </p>
      )}
    </Inspector>
  )
}
