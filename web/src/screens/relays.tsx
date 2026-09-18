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
import { candidateCount, REACHABILITY_STATES, shortId } from "./derive"

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
          {runtime.privateRelayCandidates.length === 0 &&
          runtime.publicRelayFallbacks.length === 0 ? (
            <EmptyState title="No candidate supplied">
              No Private Relay advertisement and no configured public fallback are present in this
              snapshot.
            </EmptyState>
          ) : (
            <>
              {runtime.privateRelayCandidates.length === 0 ? null : (
                <div className="scroll-x">
                  <CoverageMatrix
                    caption="MA2A Private Relay coverage by Space"
                    rows={runtime.privateRelayCandidates.map((relay) => ({
                      id: relay.providerEndpointId,
                      name: relay.relayUrl,
                      detail: `provider ${shortId(relay.providerEndpointId)}`,
                      covers: runtime.spaces.map((space) =>
                        relay.coveredSpaceIds.includes(space.id),
                      ),
                      compatible: relay.homeCompatible,
                    }))}
                    spaces={runtime.spaces.map((space) => space.name)}
                  />
                </div>
              )}
              {runtime.publicRelayFallbacks.length === 0 ? null : (
                <div>
                  <span className="eyebrow">Public Iroh relay fallback</span>
                  <ul className="peer-list">
                    {runtime.publicRelayFallbacks.map((fallback) => (
                      <li key={fallback.relayUrl}>
                        <span className="peer peer--static">
                          <span className="peer__id">{fallback.relayUrl}</span>
                          <span className="peer__state">
                            external transport, no Endpoint identity
                          </span>
                          <Pill filled tone={fallback.observedConnected ? "direct" : "none"}>
                            {fallback.observedConnected ? "connected" : "not connected"}
                          </Pill>
                        </span>
                      </li>
                    ))}
                  </ul>
                </div>
              )}
            </>
          )}
          <p className="field__help">
            MA2A supplies Iroh {candidateCount(runtime)} of these entries: the home-compatible
            Private Relays plus every enabled public fallback. A Private Relay is a role hosted by
            an MA2A Endpoint and is home-compatible only when it covers every active Space. A public
            Iroh relay is external infrastructure with no Endpoint identity and no Space coverage. A
            listed candidate is not a reachability guarantee.
          </p>
        </Card>
        <Card label="Effective: what Iroh reports back">
          <MeterList
            label="Observed relay state"
            meters={[
              {
                label: "Public relay connected",
                fraction: runtime.observedRelayState.publicRelayConnected ? 1 : 0,
                value: runtime.observedRelayState.publicRelayConnected ? "yes" : "no",
                tone: runtime.observedRelayState.publicRelayConnected ? "direct" : "none",
                note: "An Iroh transport observation for this Endpoint.",
              },
              {
                label: "Private Relay Provider running here",
                fraction: runtime.observedRelayState.privateRelayProviderRunning ? 1 : 0,
                value: runtime.observedRelayState.privateRelayProviderRunning ? "yes" : "no",
                tone: runtime.observedRelayState.privateRelayProviderRunning ? "accent" : "none",
                note: "A service this Runtime hosts for others. It says nothing about whether this Endpoint has a connected home relay.",
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
