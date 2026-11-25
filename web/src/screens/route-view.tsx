import type { ReactNode } from "react"

import type { PendingSpaceCreation } from "../api/pending-space"
import { ROUTE_PATHS, type RoutePath } from "../routes"
import type { RuntimeActions } from "../runtime-actions"
import type { RuntimeViewData } from "../view-model"
import { EchoInspector, EchoScreen } from "./echo"
import { MapInspector, MapScreen } from "./map"
import { PeerInspector, PeersScreen } from "./peers"
import { RelaysInspector } from "./relay-config"
import { RelaysScreen } from "./relays"
import { ServicesInspector, ServicesScreen, type ServicesState } from "./services"
import { SettingsInspector, SettingsScreen } from "./settings"
import { SpacesInspector, SpacesScreen } from "./spaces"

export type RouteViewOptions = {
  readonly path: RoutePath
  readonly runtime: RuntimeViewData | undefined
  readonly services: ServicesState
  readonly actions: RuntimeActions | undefined
  readonly pendingSpaceCreation?: PendingSpaceCreation | undefined
  readonly selectedPeer: string | undefined
  readonly syncMessage: string | undefined
  readonly onSelectPeer: (id: string) => void
  readonly onSync: (id: string) => void
  readonly onNavigate: (path: RoutePath) => void
  readonly onLogout: (() => Promise<void>) | undefined
  readonly onSessionsRevoked: (() => void) | undefined
}

export type RouteView = {
  readonly content: ReactNode
  readonly inspector: ReactNode
}

/** Every runtime route is a wide evidence region plus a narrow action rail. */
export function routeView(options: RouteViewOptions): RouteView {
  const { path, runtime, actions, selectedPeer } = options
  switch (path) {
    case ROUTE_PATHS.spaces:
      return {
        content: <SpacesScreen runtime={runtime} />,
        inspector: (
          <SpacesInspector
            actions={actions}
            pendingSpaceCreation={options.pendingSpaceCreation}
            runtime={runtime}
          />
        ),
      }
    case ROUTE_PATHS.endpoint:
      return {
        content: (
          <PeersScreen onSelect={options.onSelectPeer} runtime={runtime} selected={selectedPeer} />
        ),
        inspector: (
          <PeerInspector
            actions={actions}
            message={options.syncMessage}
            onSync={options.onSync}
            runtime={runtime}
            selected={selectedPeer}
          />
        ),
      }
    case ROUTE_PATHS.relays:
      return {
        content: <RelaysScreen runtime={runtime} />,
        inspector: <RelaysInspector actions={actions} />,
      }
    case ROUTE_PATHS.services:
      return {
        content: <ServicesScreen runtime={runtime} services={options.services} />,
        inspector: <ServicesInspector />,
      }
    case ROUTE_PATHS.echo:
      return {
        content: <EchoScreen actions={actions} runtime={runtime} />,
        inspector: <EchoInspector runtime={runtime} />,
      }
    case ROUTE_PATHS.settings:
      return {
        content: <SettingsScreen runtime={runtime} />,
        inspector: (
          <SettingsInspector
            actions={actions}
            onLogout={options.onLogout}
            onSessionsRevoked={options.onSessionsRevoked}
            runtime={runtime}
          />
        ),
      }
    default:
      return {
        content: (
          <MapScreen onSelect={options.onSelectPeer} runtime={runtime} selected={selectedPeer} />
        ),
        inspector: (
          <MapInspector
            actions={actions}
            onCreateSpace={() => options.onNavigate(ROUTE_PATHS.spaces)}
            runtime={runtime}
          />
        ),
      }
  }
}
