import { type ReactNode, useState } from "react"

import { EmptyState, PendingSnapshot } from "../components/feedback"
import { Field, Form, text } from "../components/form"
import { Inspector } from "../components/inspector"
import { Card, Pill, Section } from "../components/ui"
import type { RuntimeActions } from "../runtime-actions"
import type { RuntimeViewData } from "../view-model"
import { CoverageMatrix } from "../viz/coverage-matrix"
import { MeterList } from "../viz/meter-list"
import { StateMachine } from "../viz/state-machine"
import { REACHABILITY_STATES, shortId } from "./derive"

export function RelaysScreen({
  runtime,
}: {
  readonly runtime: RuntimeViewData | undefined
}): ReactNode {
  if (runtime === undefined) return <PendingSnapshot />
  return (
    <Section
      description="MA2A filters candidates and hands them to Iroh. Iroh alone probes, selects a home and upgrades to direct. These are separate facts and stay on separate rows."
      title="Relays"
    >
      <Card label="Reachability, as the Runtime names it">
        <StateMachine current={runtime.reachability.state} states={REACHABILITY_STATES} />
        <p className="field__help">{runtime.reachability.detail}</p>
      </Card>
      <div className="split">
        <Card label="Desired: candidates supplied to Iroh">
          {runtime.relays.length === 0 ? (
            <EmptyState title="No candidate supplied">
              No compatible Private Relay and no enabled Public fallback are present in this
              snapshot.
            </EmptyState>
          ) : (
            <div className="scroll-x">
              <CoverageMatrix
                caption="Relay candidate coverage by Space"
                rows={runtime.relays.map((relay) => ({
                  id: relay.endpointId,
                  name: relay.kind === "private" ? "Private Relay" : "Public fallback",
                  detail: shortId(relay.endpointId),
                  covers: runtime.spaces.map((space) => relay.coveredSpaceIds.includes(space.id)),
                  compatible: relay.status === "eligible",
                }))}
                spaces={runtime.spaces.map((space) => space.name)}
              />
            </div>
          )}
          <p className="field__help">
            A candidate is home-compatible exactly when it covers every active Space. A listed
            candidate is still not a reachability guarantee.
          </p>
        </Card>
        <Card label="Effective: what Iroh reports back">
          <MeterList
            label="Observed relay state"
            meters={[
              {
                label: "Private relay online",
                fraction: runtime.observedRelayState.private_relay_online ? 1 : 0,
                value: runtime.observedRelayState.private_relay_online ? "yes" : "no",
                tone: runtime.observedRelayState.private_relay_online ? "direct" : "none",
              },
              {
                label: "Public relay online",
                fraction: runtime.observedRelayState.public_relay_online ? 1 : 0,
                value: runtime.observedRelayState.public_relay_online ? "yes" : "no",
                tone: runtime.observedRelayState.public_relay_online ? "direct" : "none",
              },
            ]}
          />
          <div className="cluster">
            <Pill tone="none">aggregate path: {runtime.endpoint.observedPath}</Pill>
          </div>
          <p className="field__help">
            Cleared at every restart, then rebuilt from the running Endpoint. MA2A never claims to
            pick Iroh&apos;s home relay.
          </p>
        </Card>
      </div>
    </Section>
  )
}

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
            help="Separate Space IDs with commas or line breaks. Effective service is the intersection with the Spaces where you still hold the relay-provider capability."
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
