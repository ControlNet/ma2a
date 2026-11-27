import type { RequestId } from "./api/generated"
import type {
  MutationEchoReplyView,
  MutationPrivateRelayView,
  MutationPublicRelayView,
  PrivateRelayConfiguration,
  RuntimeMutationClient,
} from "./api/mutations"
import { sessionIsGone } from "./api/web-auth"

export type RuntimeActions = {
  readonly createSpace: (name: string) => Promise<void>
  readonly discardPendingSpaceCreation: (requestId: RequestId) => void
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

/**
 * The console's commands. One refused because its session is gone reports `onSessionExpired`
 * and still fails, as the console signs out only on that answer.
 */
export function createRuntimeActions(
  client: RuntimeMutationClient,
  refresh: () => Promise<void>,
  onSessionExpired: () => void,
): RuntimeActions {
  const send = async <Value>(operation: () => Promise<Value>): Promise<Value> => {
    try {
      return await operation()
    } catch (error) {
      if (sessionIsGone(error)) onSessionExpired()
      throw error
    }
  }
  const mutate = async <Value>(operation: () => Promise<Value>): Promise<Value> => {
    const value = await send(operation)
    await refresh()
    return value
  }
  return {
    createSpace: async (name) => void (await mutate(() => client.createSpace(name))),
    discardPendingSpaceCreation: (requestId) => client.discardPendingSpaceCreation(requestId),
    createInvitation: async (spaceId, ttlMs, outputPath) =>
      void (await mutate(() => client.createInvitation(spaceId, ttlMs, outputPath))),
    revokeEndpoint: async (spaceId, endpointId) =>
      void (await mutate(() => client.revokeEndpoint(spaceId, endpointId))),
    triggerSync: async (endpointId) => void (await mutate(() => client.triggerSync(endpointId))),
    configurePrivateRelay: (configuration) =>
      mutate(() => client.configurePrivateRelay(configuration)),
    configurePublicRelay: (url) => mutate(() => client.configurePublicRelay(url)),
    echo: (endpointId, payload) => mutate(() => client.echo(endpointId, payload)),
    revokeSessions: async () => void (await send(() => client.revokeSessions())),
  }
}
