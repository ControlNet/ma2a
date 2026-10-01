import type { ReactNode } from "react"

import type { HostedService, HostedServicesView } from "../api/services-hosted"
import { EmptyState } from "../components/feedback"
import { Card } from "../components/ui"

/** Calls admitted per version since the Runtime started, as `v1: 3, v2: 5`. */
export function callCounts(service: HostedService): string {
  if (service.calls.length === 0) return "none yet"
  return service.calls.map((calls) => `v${calls.version}: ${calls.count}`).join(", ")
}

function HostedServiceTable({
  services,
}: {
  readonly services: readonly HostedService[]
}): ReactNode {
  return (
    <table className="ops-table">
      <caption className="visually-hidden">Services this Runtime hosts</caption>
      <thead>
        <tr>
          <th scope="col">Service</th>
          <th scope="col">Description</th>
          <th scope="col">State</th>
          <th scope="col">Versions</th>
          <th scope="col">Running</th>
          <th scope="col">Calls</th>
        </tr>
      </thead>
      <tbody>
        {services.map((service) => (
          <tr key={service.service}>
            <th scope="row">{service.service}</th>
            <td>{service.description}</td>
            <td>{service.state}</td>
            <td>{service.versions.map((version) => version.version).join(", ")}</td>
            <td>{`${service.running_calls} of ${service.max_calls}`}</td>
            <td>{callCounts(service)}</td>
          </tr>
        ))}
      </tbody>
    </table>
  )
}

/** Hosted services: what each offers and how it is doing; calls are made through the CLI. */
export function HostedServicesCard({ hosted }: { readonly hosted: HostedServicesView }): ReactNode {
  const more = hosted.more ? " (first page)" : ""
  return (
    <Card label={`Hosted services · ${hosted.services.length}${more}`}>
      {hosted.services.length === 0 ? (
        <EmptyState title="No hosted service">
          The operator registers services from the trusted terminal with{" "}
          <code>ma2a service register</code>.
        </EmptyState>
      ) : (
        <div className="scroll-x">
          <HostedServiceTable services={hosted.services} />
        </div>
      )}
    </Card>
  )
}
