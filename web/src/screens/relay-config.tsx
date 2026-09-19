import { type ReactNode, useState } from "react"

import { Field, Form, text } from "../components/form"
import { Inspector } from "../components/inspector"
import { Card } from "../components/ui"
import type { RuntimeActions } from "../runtime-actions"

export function RelaysInspector({
  actions,
}: {
  readonly actions: RuntimeActions | undefined
}): ReactNode {
  const [message, setMessage] = useState<string | undefined>()
  const [native, setNative] = useState(true)
  const disabled = actions === undefined
  return (
    <Inspector eyebrow="Operations" title="Relay configuration">
      <Card label="Private provider">
        <Form
          disabled={disabled}
          label="Configure the Private Relay"
          onSubmit={(data) => {
            if (actions === undefined) return
            setMessage("Configuring…")
            const served = text(data, "served")
              .split(/[\s,]+/u)
              .filter((value) => value.length > 0)
            const base = {
              listen: text(data, "listen"),
              publicUrl: text(data, "public-url"),
              servedSpaceIds: served,
            }
            const configuration = native
              ? {
                  mode: "native_tls" as const,
                  ...base,
                  certificatePath: text(data, "cert"),
                  privateKeyPath: text(data, "key"),
                }
              : {
                  mode: "external_termination" as const,
                  ...base,
                  certificatePath: null,
                  privateKeyPath: null,
                }
            void actions.configurePrivateRelay(configuration).then(
              (relay) => setMessage(`Configured at ${relay.host}:${relay.port}.`),
              () => setMessage("The Runtime rejected the configuration."),
            )
          }}
          submitLabel="Configure private"
          {...(message === undefined ? {} : { status: message })}
        >
          <fieldset className="segmented">
            <legend className="visually-hidden">TLS termination</legend>
            <button
              aria-pressed={native}
              className="segmented__option"
              onClick={() => setNative(true)}
              type="button"
            >
              Native TLS
            </button>
            <button
              aria-pressed={!native}
              className="segmented__option"
              onClick={() => setNative(false)}
              type="button"
            >
              External
            </button>
          </fieldset>
          <Field id="relay-listen" label="Listen address">
            <input id="relay-listen" name="listen" required />
          </Field>
          <Field id="relay-url" label="Public HTTPS URL">
            <input id="relay-url" name="public-url" pattern="https://.*" required type="url" />
          </Field>
          <Field
            help="Separate Space IDs with commas or line breaks. The Runtime may serve only active Spaces it currently belongs to and that permit Private Relay Provider operation."
            id="relay-served"
            label="Served Space IDs"
          >
            <textarea id="relay-served" name="served" required rows={2} />
          </Field>
          {native ? (
            <>
              <Field id="relay-cert" label="TLS certificate path">
                <input id="relay-cert" name="cert" required />
              </Field>
              <Field
                help="Only the path is sent to the daemon. PEM bytes never enter the browser, the local API or logs."
                id="relay-key"
                label="TLS private key path"
              >
                <input id="relay-key" name="key" required />
              </Field>
            </>
          ) : (
            <p className="field__help">
              External termination starts a plaintext backend on loopback only. Your proxy must
              terminate TLS and forward the relay upgrade unchanged.
            </p>
          )}
        </Form>
      </Card>
      <Card label="Public fallback">
        <Form
          disabled={disabled}
          label="Configure the public fallback"
          onSubmit={(data) => {
            if (actions === undefined) return
            setMessage("Configuring…")
            void actions.configurePublicRelay(text(data, "url")).then(
              () => setMessage("Public fallback updated."),
              () => setMessage("The Runtime rejected the configuration."),
            )
          }}
          submitLabel="Configure public"
        >
          <Field
            help="Fallback is explicit. MA2A never promotes it automatically."
            id="public-url"
            label="HTTPS relay URL"
          >
            <input id="public-url" name="url" pattern="https://.*" required type="url" />
          </Field>
        </Form>
      </Card>
    </Inspector>
  )
}
