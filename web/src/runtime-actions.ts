import type {
  MutationEchoReplyView,
  MutationPrivateRelayView,
  MutationPublicRelayView,
  RuntimeMutationClient,
} from "./api/mutations"

export type RuntimeActions = {
  readonly createSpace: (name: string) => Promise<void>
  readonly inviteEndpoint: (spaceId: string, endpointId: string) => Promise<void>
  readonly revokeEndpoint: (spaceId: string, endpointId: string) => Promise<void>
  readonly triggerSync: (endpointId: string) => Promise<void>
  readonly configurePrivateRelay: (
    mode: "native_tls" | "external_termination",
    host: string,
    port: number,
  ) => Promise<MutationPrivateRelayView>
  readonly configurePublicRelay: (url: string) => Promise<MutationPublicRelayView>
  readonly echo: (endpointId: string, payload: string) => Promise<MutationEchoReplyView>
  readonly revokeSessions: () => Promise<void>
}

export function createRuntimeActions(
  client: RuntimeMutationClient,
  refresh: () => Promise<void>,
): RuntimeActions {
  const mutate = async <Value>(operation: () => Promise<Value>): Promise<Value> => {
    const value = await operation()
    await refresh()
    return value
  }
  return {
    createSpace: async (name) => void (await mutate(() => client.createSpace(name))),
    inviteEndpoint: async (spaceId, endpointId) =>
      void (await mutate(() => client.inviteEndpoint(spaceId, endpointId))),
    revokeEndpoint: async (spaceId, endpointId) =>
      void (await mutate(() => client.revokeEndpoint(spaceId, endpointId))),
    triggerSync: async (endpointId) => void (await mutate(() => client.triggerSync(endpointId))),
    configurePrivateRelay: (mode, host, port) =>
      mutate(() => client.configurePrivateRelay(mode, host, port)),
    configurePublicRelay: (url) => mutate(() => client.configurePublicRelay(url)),
    echo: (endpointId, payload) => mutate(() => client.echo(endpointId, payload)),
    revokeSessions: async () => void (await client.revokeSessions()),
  }
}
