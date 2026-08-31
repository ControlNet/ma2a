import { type MouseEvent, type ReactNode, useEffect, useRef } from "react"

import { type RoutePath, RUNTIME_ROUTES, type RuntimeRoutePath } from "../routes"
import type { RuntimeViewData } from "../view-model"
import { RuntimeBanner } from "./runtime-status"

function focusMainContent(): void {
  document.getElementById("main-content")?.focus()
}

export function AppShell({
  path,
  runtime,
  onNavigate,
  children,
}: {
  readonly path: RuntimeRoutePath
  readonly runtime: RuntimeViewData | undefined
  readonly onNavigate: (path: RoutePath) => void
  readonly children: ReactNode
}): ReactNode {
  const navigationRef = useRef<HTMLElement>(null)
  useEffect(() => {
    const navigation = navigationRef.current
    const activeRoute = navigation?.querySelector<HTMLElement>(`a[href="${path}"]`)
    if (navigation === null || activeRoute === undefined || activeRoute === null) return
    navigation.scrollLeft = Math.max(
      0,
      activeRoute.offsetLeft - (navigation.clientWidth - activeRoute.clientWidth) / 2,
    )
  }, [path])
  const navigate = (event: MouseEvent<HTMLAnchorElement>, target: RoutePath): void => {
    event.preventDefault()
    onNavigate(target)
  }

  return (
    <div className="app-shell">
      {/* biome-ignore lint/a11y/useValidAnchor: Skip navigation requires anchor semantics. */}
      <a className="skip-link" href="#main-content" onClick={focusMainContent}>
        Skip to content
      </a>
      <aside className="app-sidebar">
        <a className="brand" href="/" onClick={(event) => navigate(event, "/")}>
          <span aria-hidden="true" className="brand-mark">
            M2
          </span>
          <span>
            <strong>MA2A</strong>
            <small>Runtime Console</small>
          </span>
        </a>
        <div className="route-navigation">
          <nav aria-describedby="route-scroll-hint" aria-label="Runtime" ref={navigationRef}>
            {RUNTIME_ROUTES.map((route) => (
              <a
                aria-current={path === route.path ? "page" : undefined}
                href={route.path}
                key={route.path}
                onClick={(event) => navigate(event, route.path)}
              >
                {route.label}
              </a>
            ))}
          </nav>
          <p className="route-scroll-hint" id="route-scroll-hint">
            {RUNTIME_ROUTES.map((route) =>
              path === route.path ? <strong key={route.path}>Current: {route.label}</strong> : null,
            )}{" "}
            <span>Scroll for more routes</span>
          </p>
        </div>
        <div className="sidebar-foot">
          <span>Local Runtime</span>
          <strong>{runtime?.connection ?? "waiting"}</strong>
        </div>
      </aside>
      <div className="workspace">
        {runtime === undefined ? null : <RuntimeBanner runtime={runtime} />}
        {/* biome-ignore lint/a11y/noNoninteractiveTabindex: This landmark owns keyboard scrolling. */}
        <main aria-label="Runtime content" id="main-content" tabIndex={0}>
          {children}
        </main>
      </div>
    </div>
  )
}
