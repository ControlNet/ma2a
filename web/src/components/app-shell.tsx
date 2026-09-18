import type { ReactNode } from "react"

import type { RoutePath } from "../routes"
import type { RuntimeViewData } from "../view-model"
import { IconRail } from "./icon-rail"
import { TopStrip } from "./top-strip"

function focusMain(): void {
  document.getElementById("main-content")?.focus()
}

export function AppShell({
  path,
  runtime,
  onNavigate,
  inspector,
  children,
}: {
  readonly path: RoutePath
  readonly runtime: RuntimeViewData | undefined
  readonly onNavigate: (path: RoutePath) => void
  readonly inspector?: ReactNode
  readonly children: ReactNode
}): ReactNode {
  return (
    <div className={inspector === undefined ? "shell" : "shell shell--inspected"}>
      {/* biome-ignore lint/a11y/useValidAnchor: skip navigation requires anchor semantics */}
      <a className="skip-link" href="#main-content" onClick={focusMain}>
        Skip to content
      </a>
      <TopStrip runtime={runtime} />
      <div className="shell__body">
        <IconRail onNavigate={onNavigate} path={path} />
        {/* biome-ignore lint/a11y/noNoninteractiveTabindex: this landmark owns keyboard scrolling */}
        <main aria-label="Runtime content" className="shell__main" id="main-content" tabIndex={0}>
          {children}
        </main>
        {inspector}
      </div>
    </div>
  )
}
