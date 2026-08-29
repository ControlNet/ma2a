import { type ReactNode, useCallback, useEffect, useState } from "react"

import {
  currentWebSession,
  loginAndTouchSession,
  logoutSession,
  type WebSession,
} from "./api/web-auth"
import { AppShell } from "./components/app-shell"
import { isRoutePath, ROUTE_PATHS, type RoutePath } from "./routes"
import { LoginScreen, SetupScreen } from "./screens/auth"
import { EchoScreen } from "./screens/echo"
import { EndpointScreen } from "./screens/endpoint"
import { OverviewScreen } from "./screens/overview"
import { RelaysScreen } from "./screens/relays"
import { SettingsScreen } from "./screens/settings"
import { SpacesScreen } from "./screens/spaces"
import type { RuntimeViewData } from "./view-model"

function routeContent(path: RoutePath, runtime?: RuntimeViewData): ReactNode {
  switch (path) {
    case ROUTE_PATHS.login:
      return <LoginScreen />
    case ROUTE_PATHS.setup:
      return <SetupScreen />
    case ROUTE_PATHS.overview:
      return <OverviewScreen runtime={runtime} />
    case ROUTE_PATHS.endpoint:
      return <EndpointScreen runtime={runtime} />
    case ROUTE_PATHS.spaces:
      return <SpacesScreen runtime={runtime} />
    case ROUTE_PATHS.relays:
      return <RelaysScreen runtime={runtime} />
    case ROUTE_PATHS.echo:
      return <EchoScreen runtime={runtime} />
    case ROUTE_PATHS.settings:
      return <SettingsScreen />
  }
}

function browserPath(): RoutePath {
  return isRoutePath(window.location.pathname) ? window.location.pathname : ROUTE_PATHS.overview
}

export function App({
  initialPath,
  runtime,
}: {
  readonly initialPath?: RoutePath
  readonly runtime?: RuntimeViewData
}): ReactNode {
  const [path, setPath] = useState<RoutePath>(initialPath ?? browserPath)
  const [session, setSession] = useState<WebSession | undefined>(currentWebSession)
  const controlled = initialPath !== undefined

  useEffect(() => {
    if (controlled) {
      return
    }
    const syncPath = (): void => setPath(browserPath())
    window.addEventListener("popstate", syncPath)
    return () => window.removeEventListener("popstate", syncPath)
  }, [controlled])

  const navigate = useCallback(
    (target: RoutePath): void => {
      if (!controlled) {
        window.history.pushState(null, "", target)
      }
      setPath(target)
    },
    [controlled],
  )

  const login = useCallback(
    async (passphrase: string): Promise<void> => {
      const authenticatedSession = await loginAndTouchSession(passphrase)
      setSession(authenticatedSession)
      navigate(ROUTE_PATHS.overview)
    },
    [navigate],
  )

  const logout =
    session === undefined
      ? undefined
      : async (): Promise<void> => {
          await logoutSession(session)
          setSession(undefined)
          navigate(ROUTE_PATHS.login)
        }

  if (path === ROUTE_PATHS.login) {
    return <LoginScreen onLogin={login} />
  }
  if (path === ROUTE_PATHS.setup) {
    return <SetupScreen />
  }
  return (
    <AppShell onNavigate={navigate} path={path} runtime={runtime}>
      {path === ROUTE_PATHS.settings ? (
        <SettingsScreen onLogout={logout} />
      ) : (
        routeContent(path, runtime)
      )}
    </AppShell>
  )
}
