import { type ReactNode, useCallback, useEffect, useMemo, useState } from "react"
import { createRuntimeMutationClient } from "./api/mutations"
import {
  currentWebSession,
  loginAndTouchSession,
  logoutSession,
  type WebSession,
} from "./api/web-auth"
import { AppShell } from "./components/app-shell"
import { RuntimeLoadError } from "./components/runtime-status"
import { isRoutePath, ROUTE_PATHS, type RoutePath } from "./routes"
import { createRuntimeActions, type RuntimeActions } from "./runtime-actions"
import { RuntimeController } from "./runtime-controller"
import { LoginScreen, SetupScreen } from "./screens/auth"
import { EchoScreen } from "./screens/echo"
import { EndpointScreen } from "./screens/endpoint"
import { OverviewScreen } from "./screens/overview"
import { RelaysScreen } from "./screens/relays"
import { SettingsScreen } from "./screens/settings"
import { SpacesScreen } from "./screens/spaces"
import type { RuntimeViewData } from "./view-model"

function routeContent(
  path: RoutePath,
  runtime: RuntimeViewData | undefined,
  actions: RuntimeActions | undefined,
): ReactNode {
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
      return <SpacesScreen actions={actions} runtime={runtime} />
    case ROUTE_PATHS.relays:
      return <RelaysScreen actions={actions} runtime={runtime} />
    case ROUTE_PATHS.echo:
      return <EchoScreen actions={actions} runtime={runtime} />
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
  const [liveRuntime, setLiveRuntime] = useState<RuntimeViewData | undefined>(runtime)
  const [runtimeLoadFailed, setRuntimeLoadFailed] = useState(false)
  const [controller, setController] = useState<RuntimeController | undefined>()
  const controlled = initialPath !== undefined
  const displayedRuntime = controlled ? runtime : liveRuntime

  const actions = useMemo(() => {
    if (session === undefined || controller === undefined) return undefined
    return createRuntimeActions(createRuntimeMutationClient({ csrfToken: session.csrfToken }), () =>
      controller.refresh(),
    )
  }, [controller, session])

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

  useEffect(() => {
    if (controlled || session === undefined) return
    const active = new RuntimeController({
      onRuntime: (nextRuntime) => {
        setRuntimeLoadFailed(false)
        setLiveRuntime(nextRuntime)
      },
      onSessionExpired: () => {
        setSession(undefined)
        setLiveRuntime(undefined)
        navigate(ROUTE_PATHS.login)
      },
      onError: () => setRuntimeLoadFailed(true),
    })
    setController(active)
    void active.start()
    return () => active.stop()
  }, [controlled, navigate, session])

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
          controller?.stop()
          setSession(undefined)
          setLiveRuntime(undefined)
          navigate(ROUTE_PATHS.login)
        }

  const sessionsRevoked = (): void => {
    controller?.stop()
    setSession(undefined)
    setLiveRuntime(undefined)
    setRuntimeLoadFailed(false)
    navigate(ROUTE_PATHS.login)
  }

  if (path === ROUTE_PATHS.login) {
    return <LoginScreen onLogin={login} />
  }
  if (path === ROUTE_PATHS.setup) {
    return <SetupScreen />
  }
  return (
    <AppShell onNavigate={navigate} path={path} runtime={displayedRuntime}>
      {path === ROUTE_PATHS.settings ? (
        <SettingsScreen
          actions={actions}
          onLogout={logout}
          onSessionsRevoked={sessionsRevoked}
          runtime={displayedRuntime}
        />
      ) : displayedRuntime === undefined && runtimeLoadFailed ? (
        <RuntimeLoadError onRetry={() => void controller?.refresh()} />
      ) : (
        routeContent(path, displayedRuntime, actions)
      )}
    </AppShell>
  )
}
