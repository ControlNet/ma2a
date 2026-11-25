import { type ReactNode, useCallback, useEffect, useMemo, useState } from "react"
import { createRuntimeMutationClient } from "./api/mutations"
import type { ServicesData } from "./api/services"
import {
  currentWebSession,
  loginAndTouchSession,
  logoutSession,
  sessionIsGone,
  type WebSession,
} from "./api/web-auth"
import { AppShell } from "./components/app-shell"
import { SnapshotError } from "./components/feedback"
import { isRoutePath, ROUTE_PATHS, type RoutePath } from "./routes"
import { createRuntimeActions } from "./runtime-actions"
import { RuntimeController } from "./runtime-controller"
import { LoginScreen, SetupScreen } from "./screens/auth"
import { routeView } from "./screens/route-view"
import { useServices } from "./screens/use-services"
import type { RuntimeViewData } from "./view-model"

function browserPath(): RoutePath {
  return isRoutePath(window.location.pathname) ? window.location.pathname : ROUTE_PATHS.overview
}

export function App({
  initialPath,
  runtime,
  services,
}: {
  readonly initialPath?: RoutePath
  readonly runtime?: RuntimeViewData
  readonly services?: ServicesData
}): ReactNode {
  const [path, setPath] = useState<RoutePath>(initialPath ?? browserPath)
  const [session, setSession] = useState<WebSession | undefined>(currentWebSession)
  const [liveRuntime, setLiveRuntime] = useState<RuntimeViewData | undefined>(runtime)
  const [runtimeLoadFailed, setRuntimeLoadFailed] = useState(false)
  const [controller, setController] = useState<RuntimeController | undefined>()
  const [selectedPeer, setSelectedPeer] = useState<string | undefined>()
  const [syncStatus, setSyncStatus] = useState<
    { readonly endpointId: string; readonly text: string } | undefined
  >()
  const controlled = initialPath !== undefined
  const displayedRuntime = controlled ? runtime : liveRuntime
  const liveServices = useServices(
    !controlled && session !== undefined && path === ROUTE_PATHS.services,
  )
  const displayedServices = controlled
    ? services === undefined
      ? ({ status: "pending" } as const)
      : ({ status: "ready", data: services } as const)
    : liveServices

  const actions = useMemo(() => {
    if (session === undefined || controller === undefined) return undefined
    const csrfToken = (): string => currentWebSession()?.csrfToken ?? session.csrfToken
    return createRuntimeActions(createRuntimeMutationClient({ csrfToken }), () =>
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

  // Every way a session ends clears what the last one loaded, so the next sign-in starts clean.
  const signedOut = useCallback((): void => {
    setSession(undefined)
    setLiveRuntime(undefined)
    setRuntimeLoadFailed(false)
    setSelectedPeer(undefined)
    // Clearing the request also stops a late answer to it from installing itself.
    setSyncStatus(undefined)
    navigate(ROUTE_PATHS.login)
  }, [navigate])

  useEffect(() => {
    if (controlled || session === undefined) return
    const active = new RuntimeController({
      onRuntime: (nextRuntime) => {
        setRuntimeLoadFailed(false)
        setLiveRuntime(nextRuntime)
      },
      onSessionExpired: signedOut,
      onError: () => setRuntimeLoadFailed(true),
    })
    setController(active)
    void active.start()
    return () => active.stop()
  }, [controlled, session, signedOut])

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
          try {
            await logoutSession(session)
          } catch (error) {
            if (!sessionIsGone(error)) throw error
          }
          controller?.stop()
          signedOut()
        }

  const sessionsRevoked = (): void => {
    controller?.stop()
    signedOut()
  }

  if (path === ROUTE_PATHS.setup) {
    return <SetupScreen />
  }
  // No console page can load without a session, as after going back from the sign-in screen.
  if (path === ROUTE_PATHS.login || (!controlled && session === undefined)) {
    return <LoginScreen onLogin={login} />
  }

  const sync = (endpointId: string): void => {
    if (actions === undefined) return
    const requested = { endpointId, text: "Requesting a bounded control round\u2026" }
    setSyncStatus(requested)
    // Only the latest request reports its answer, so a late answer never replaces a newer one.
    const settle = (text: string): void =>
      setSyncStatus((current) => (current === requested ? { endpointId, text } : current))
    void actions.triggerSync(endpointId).then(
      () => settle("Control round requested."),
      () => settle("The Runtime rejected the request."),
    )
  }

  const view = routeView({
    path,
    runtime: displayedRuntime,
    services: displayedServices,
    actions,
    selectedPeer,
    syncMessage: syncStatus?.endpointId === selectedPeer ? syncStatus?.text : undefined,
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
