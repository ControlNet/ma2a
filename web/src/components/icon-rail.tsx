import type { MouseEvent, ReactNode } from "react"

import { type RoutePath, RUNTIME_ROUTES } from "../routes"
import { Icon } from "./icons"

/**
 * Six destinations, all of them visible at every width: a vertical rail on the
 * desktop and a bottom tab bar on a phone. Nothing scrolls, so nothing has to
 * be scrolled into view and the sequential focus origin never moves.
 */
export function IconRail({
  path,
  onNavigate,
}: {
  readonly path: RoutePath
  readonly onNavigate: (path: RoutePath) => void
}): ReactNode {
  const navigate = (event: MouseEvent<HTMLAnchorElement>, target: RoutePath): void => {
    event.preventDefault()
    onNavigate(target)
  }
  return (
    <nav aria-label="Console sections" className="rail">
      {RUNTIME_ROUTES.map((route) => (
        <a
          aria-current={path === route.path ? "page" : undefined}
          className="rail__link"
          href={route.path}
          key={route.path}
          onClick={(event) => navigate(event, route.path)}
        >
          <Icon name={route.icon} />
          <span className="rail__label">{route.label}</span>
        </a>
      ))}
    </nav>
  )
}
