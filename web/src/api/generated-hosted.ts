import type { EndpointId, LOCAL_API_VERSION, RequestId } from "./generated"

type Versioned = { readonly version: typeof LOCAL_API_VERSION }
type Mutation = Versioned & { readonly request_id: RequestId }

export type ServiceInteraction = "unary" | "stream" | "bytes" | "resource"

export type ServiceOperationView = {
  readonly name: string
  readonly interaction: ServiceInteraction
}

export type ServiceVersionView = {
  readonly version: number
  readonly operations: readonly ServiceOperationView[]
}

/** What a hosted service offers: its versions, their operations, and its call bound. */
export type ServiceManifestView = {
  readonly description: string
  readonly max_calls: number
  readonly versions: readonly ServiceVersionView[]
}

/** Phase 5 local commands on the services this Runtime hosts. */
export type HostedServiceCommand =
  | (Mutation & {
      readonly operation: "service_register"
      readonly service: string
      readonly manifest: ServiceManifestView
    })
  | (Mutation & { readonly operation: "service_unregister"; readonly service: string })
  | (Mutation & { readonly operation: "service_start"; readonly service: string })
  | (Mutation & { readonly operation: "service_stop"; readonly service: string })
  | (Versioned & { readonly operation: "service_hosted_list"; readonly after: string | null })
  | (Versioned & { readonly operation: "service_attach"; readonly service: string })
  | (Versioned & {
      readonly operation: "service_accept"
      readonly service: string
      readonly call_id: string
    })

export type HostedServiceStateName = "stopped" | "unavailable" | "ready"

export type HostedServiceCallsView = { readonly version: number; readonly count: number }

/** One registration with what runs now; counters restart with the Runtime. */
export type HostedServiceView = {
  readonly service: string
  readonly manifest: ServiceManifestView
  readonly enabled: boolean
  readonly state: HostedServiceStateName
  readonly running_calls: number
  readonly calls: readonly HostedServiceCallsView[]
  readonly created_at_ms: number
  readonly updated_at_ms: number
}

export type HostedServicePageView = {
  readonly services: readonly HostedServiceView[]
  readonly next: string | null
}

export type ServiceRegisteredView = {
  readonly service: HostedServiceView
  readonly created: boolean
}

export type ServiceUnregisteredView = { readonly service: string }

export type ServiceAttachedView = {
  readonly service: string
  readonly manifest: ServiceManifestView
}

/** One call handed to its service; `body` is standard base64. */
export type ServiceCallView = {
  readonly service: string
  readonly call_id: string
  readonly caller_endpoint_id: EndpointId
  readonly request_id: RequestId
  readonly version: number
  readonly operation: string
  readonly body: string
}
