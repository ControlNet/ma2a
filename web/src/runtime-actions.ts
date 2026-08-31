import type {
  MutationEchoReplyView,
  MutationPrivateRelayView,
  MutationPublicRelayView,
  PrivateRelayConfiguration,
  RuntimeMutationClient,
} from "./api/mutations"

export type RuntimeActions = {
  readonly createSpace: (name: string) => Promise<void>
  readonly createInvitation: (spaceId: string, ttlMs: number, outputPath: string) => Promise<void>
  readonly revokeEndpoint: (spaceId: string, endpointId: string) => Promise<void>
  readonly triggerSync: (endpointId: string) => Promise<void>
  readonly configurePrivateRelay: (
    configuration: PrivateRelayConfiguration,
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
    createInvitation: async (spaceId, ttlMs, outputPath) =>
      void (await mutate(() => client.createInvitation(spaceId, ttlMs, outputPath))),
    revokeEndpoint: async (spaceId, endpointId) =>
      void (await mutate(() => client.revokeEndpoint(spaceId, endpointId))),
    triggerSync: async (endpointId) => void (await mutate(() => client.triggerSync(endpointId))),
    configurePrivateRelay: (configuration) =>
      mutate(() => client.configurePrivateRelay(configuration)),
    configurePublicRelay: (url) => mutate(() => client.configurePublicRelay(url)),
    echo: (endpointId, payload) => mutate(() => client.echo(endpointId, payload)),
    revokeSessions: async () => void (await client.revokeSessions()),
  }
}
