import type { ReactNode } from "react"

import type {
  ActiveOperation,
  OperationRecord,
  ServiceExposure,
  ServicesData,
} from "../api/services"
import { EmptyState, PendingSnapshot } from "../components/feedback"
import { Inspector } from "../components/inspector"
import { Card, Section } from "../components/ui"
import type { RuntimeViewData } from "../view-model"
import { shortId } from "./derive"
import { byteCount, duration, exposureScope } from "./services-format"

/** Exposures and operations are read beside the snapshot, never cached in the browser. */
export type ServicesState =
  | { readonly status: "pending" }
  | { readonly status: "failed" }
  | { readonly status: "ready"; readonly data: ServicesData }

const SERVICE_NAME: Record<ServiceExposure["kind"], string> = {
  files_root: "Files",
  exec: "Exec",
  shell: "Shell",
  proxy: "Proxy",
}

function spaceName(runtime: RuntimeViewData, id: string): string {
  return runtime.spaces.find((space) => space.id === id)?.name ?? shortId(id)
}

function ExposureTable({
  runtime,
  exposures,
}: {
  readonly runtime: RuntimeViewData
  readonly exposures: readonly ServiceExposure[]
}): ReactNode {
  return (
    <table className="ops-table">
      <caption className="visually-hidden">Services this Runtime serves, by Space</caption>
      <thead>
        <tr>
          <th scope="col">Space</th>
          <th scope="col">Service</th>
          <th scope="col">Scope</th>
        </tr>
      </thead>
      <tbody>
        {exposures.map((exposure) => (
          <tr key={`${exposure.space_id}-${exposure.kind}-${exposureScope(exposure)}`}>
            <th scope="row">{spaceName(runtime, exposure.space_id)}</th>
            <td>{SERVICE_NAME[exposure.kind]}</td>
            <td className="ops-table__scope">{exposureScope(exposure)}</td>
          </tr>
        ))}
      </tbody>
    </table>
  )
}

function ActiveTable({ active }: { readonly active: readonly ActiveOperation[] }): ReactNode {
  return (
    <table className="ops-table">
      <caption className="visually-hidden">Operations running now</caption>
      <thead>
        <tr>
          <th scope="col">Peer</th>
          <th scope="col">Operation</th>
          <th scope="col">Direction</th>
          <th scope="col">Running for</th>
        </tr>
      </thead>
      <tbody>
        {active.map((operation) => (
          <tr key={`${operation.direction}-${operation.request_id}`}>
            <th className="ops-table__id" scope="row">
              {shortId(operation.peer_endpoint_id)}
            </th>
            <td>{`${operation.service} ${operation.operation}`}</td>
            <td>{operation.direction}</td>
            <td>{duration(operation.elapsed_ms)}</td>
          </tr>
        ))}
      </tbody>
    </table>
  )
}

function RecentTable({ recent }: { readonly recent: readonly OperationRecord[] }): ReactNode {
  return (
    <table className="ops-table">
      <caption className="visually-hidden">Recently finished operations, newest first</caption>
      <thead>
        <tr>
          <th scope="col">Peer</th>
          <th scope="col">Operation</th>
          <th scope="col">Result</th>
          <th scope="col">In</th>
          <th scope="col">Out</th>
          <th scope="col">Took</th>
        </tr>
      </thead>
      <tbody>
        {recent.map((record) => (
          <tr key={`${record.direction}-${record.request_id}`}>
            <th className="ops-table__id" scope="row">
              {shortId(record.peer_endpoint_id)}
            </th>
            <td>{`${record.direction} ${record.service} ${record.operation}`}</td>
            <td>{record.error === null ? record.result : `${record.result}: ${record.error}`}</td>
            <td>{byteCount(record.bytes_in)}</td>
            <td>{byteCount(record.bytes_out)}</td>
            <td>{duration(record.duration_ms)}</td>
          </tr>
        ))}
      </tbody>
    </table>
  )
}

export function ServicesScreen({
  runtime,
  services,
}: {
  readonly runtime: RuntimeViewData | undefined
  readonly services: ServicesState
}): ReactNode {
  if (runtime === undefined || services.status === "pending") return <PendingSnapshot />
  if (services.status === "failed") {
    return (
      <Section title="Services">
        <EmptyState title="Services could not be loaded">
          The Runtime did not answer. This view retries every few seconds.
        </EmptyState>
      </Section>
    )
  }
  const { exposures, operations } = services.data
  return (
    <Section title="Services">
      <Card label={`Exposures · ${exposures.length}`}>
        {exposures.length === 0 ? (
          <EmptyState title="Nothing is served">
            Grants alone serve nothing. Consent from the trusted terminal with
            <code> ma2a service expose</code>.
          </EmptyState>
        ) : (
          <div className="scroll-x">
            <ExposureTable exposures={exposures} runtime={runtime} />
          </div>
        )}
      </Card>
      <Card label={`Active operations · ${operations.active_count}`}>
        {operations.active.length === 0 ? (
          <EmptyState title="Nothing running">No operation is in progress.</EmptyState>
        ) : (
          <div className="scroll-x">
            <ActiveTable active={operations.active} />
          </div>
        )}
      </Card>
      <Card
        label={`Recent operations · latest ${operations.recent.length} of ${operations.recent_count}`}
      >
        {operations.recent.length === 0 ? (
          <EmptyState title="No finished operation">
            The audit is kept in memory and starts empty after every restart.
          </EmptyState>
        ) : (
          <div className="scroll-x">
            <RecentTable recent={operations.recent} />
          </div>
        )}
      </Card>
    </Section>
  )
}

export function ServicesInspector(): ReactNode {
  return (
    <Inspector eyebrow="Services" title="Grants and consent">
      <Card label="Two keys, both required">
        <p className="field__help">
          A Space owner signs what each member may ask for. This Runtime serves only what it has
          consented to expose in that same Space. Neither is changed from the browser.
        </p>
        <pre className="command">
          ma2a space grant SPACE_REF ENDPOINT_ID --files-read --exec{"\n"}
          ma2a service expose files SPACE_REF ROOT PATH --read-only{"\n"}
          ma2a service expose exec SPACE_REF{"\n"}
          ma2a service unexpose exec SPACE_REF
        </pre>
      </Card>
      <Card label="What is recorded">
        <p className="field__help">
          Operations record the peer, service, result, byte counts and duration. Paths, arguments,
          environment, file content, terminal and proxy bytes are never recorded.
        </p>
      </Card>
    </Inspector>
  )
}
