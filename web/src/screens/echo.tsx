import { type ReactNode, useState } from "react"
import type { MutationEchoReplyView } from "../api/mutations"
import { PendingSnapshot } from "../components/feedback"
import { Field, Form, text } from "../components/form"
import { Inspector } from "../components/inspector"
import { Card, Pill, Section } from "../components/ui"
import type { RuntimeActions } from "../runtime-actions"
import type { RuntimeViewData } from "../view-model"
import { Donut } from "../viz/donut"
import { MeterList } from "../viz/meter-list"
import { echoSegments, LIMITS, shortId } from "./derive"

export function EchoScreen({
  runtime,
  actions,
}: {
  readonly runtime: RuntimeViewData | undefined
  readonly actions: RuntimeActions | undefined
}): ReactNode {
  const [payload, setPayload] = useState("hello")
  const [result, setResult] = useState<string | undefined>()
  const [reply, setReply] = useState<MutationEchoReplyView | undefined>()
  if (runtime === undefined) return <PendingSnapshot />
  const bytes = new TextEncoder().encode(payload).length
  return (
    <Section title="Echo">
      <Card label="Request">
        <Form
          disabled={actions === undefined || bytes > LIMITS.echoPayload}
          label="Send an Echo"
          onSubmit={(data) => {
            if (actions === undefined) return
            setResult("Echo in progress…")
            setReply(undefined)
            void actions.echo(text(data, "target"), payload).then(
              (received) => {
                setReply(received)
                setResult(undefined)
              },
              () => setResult("Echo failed or was not authorized."),
            )
          }}
          submitLabel="Send Echo"
          {...(result === undefined ? {} : { status: result })}
        >
          <Field help="64 hexadecimal characters." id="echo-target" label="Target Endpoint ID">
            <input id="echo-target" name="target" pattern="[0-9a-f]{64}" required />
          </Field>
          <Field id="echo-payload" label="Payload">
            <textarea
              id="echo-payload"
              name="payload"
              onChange={(event) => setPayload(event.currentTarget.value)}
              rows={3}
              value={payload}
            />
          </Field>
          <MeterList
            label="Echo bounds"
            meters={[
              {
                label: "Payload bytes",
                fraction: bytes / LIMITS.echoPayload,
                value: `${bytes} / ${LIMITS.echoPayload}`,
                tone: bytes > LIMITS.echoPayload ? "failed" : "accent",
              },
            ]}
          />
        </Form>
        {reply === undefined ? null : (
          <div className="stack-4" role="status">
            <div className="cluster">
              <Pill filled tone="direct">
                reply from {shortId(reply.target_endpoint_id)}
              </Pill>
              <Pill tone="accent">{reply.duration_ms} ms</Pill>
            </div>
            <code className="identifier__value">{reply.payload}</code>
            <MeterList
              label="Echo deadline"
              meters={[
                {
                  label: "Aggregate deadline used",
                  fraction: reply.duration_ms / LIMITS.echoDeadlineMs,
                  value: `${reply.duration_ms} / ${LIMITS.echoDeadlineMs} ms`,
                  tone: "direct",
                },
              ]}
            />
          </div>
        )}
      </Card>
    </Section>
  )
}

export function EchoInspector({
  runtime,
}: {
  readonly runtime: RuntimeViewData | undefined
}): ReactNode {
  if (runtime === undefined) {
    return (
      <Inspector eyebrow="Echo" title="No snapshot">
        <p className="field__help">Waiting for the Runtime.</p>
      </Inspector>
    )
  }
  const echo = echoSegments(runtime)
  return (
    <Inspector eyebrow="Echo" title="Recent outcomes">
      <Card label="Bounded totals">
        <div className="cluster">
          <Donut label="echo" segments={echo.segments} value={String(echo.total)} />
          <div className="stack-4">
            <Pill filled tone="direct">
              {runtime.echoTotals.successes} ok
            </Pill>
            <Pill filled tone="failed">
              {runtime.echoTotals.failures} failed
            </Pill>
          </div>
        </div>
      </Card>
    </Inspector>
  )
}
