import { type ReactNode, useCallback, useEffect, useMemo, useState } from "react"
import { createRuntimeMutationClient } from "./api/mutations"
import {
  currentWebSession,
  loginAndTouchSession,
  logoutSession,
  type WebSession,
} from "./api/web-auth"
import { AppShell } from "./components/app-shell"
import { SnapshotError } from "./components/feedback"
import { isRoutePath, ROUTE_PATHS, type RoutePath } from "./routes"
import { createRuntimeActions } from "./runtime-actions"
import { RuntimeController } from "./runtime-controller"
import { LoginScreen, SetupScreen } from "./screens/auth"
import { routeView } from "./screens/route-view"
import type { RuntimeViewData } from "./view-model"

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
  const [selectedPeer, setSelectedPeer] = useState<string | undefined>()
  const [syncMessage, setSyncMessage] = useState<string | undefined>()
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
    async (password: string): Promise<void> => {
      const authenticatedSession = await loginAndTouchSession(password)
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

  const sync = (endpointId: string): void => {
    if (actions === undefined) return
    setSyncMessage("Requesting a bounded control round\u2026")
    void actions.triggerSync(endpointId).then(
      () => setSyncMessage("Control round requested."),
      () => setSyncMessage("The Runtime rejected the request."),
    )
  }

  const view = routeView({
    path,
    runtime: displayedRuntime,
    actions,
    selectedPeer,
    syncMessage,
    onSelectPeer: setSelectedPeer,
    onSync: sync,
    onNavigate: navigate,
    onLogout: logout,
    onSessionsRevoked: sessionsRevoked,
  })
  const unavailable = displayedRuntime === undefined && runtimeLoadFailed

  return (
    <AppShell
      inspector={unavailable ? undefined : view.inspector}
      onNavigate={navigate}
      path={path}
      runtime={displayedRuntime}
    >
      {unavailable ? <SnapshotError onRetry={() => void controller?.refresh()} /> : view.content}
    </AppShell>
  )
}
