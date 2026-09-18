import { useCallback, useEffect, useState } from "react"

import {
  applyTheme,
  DARK_QUERY,
  prefersDarkNow,
  type ResolvedTheme,
  readStoredTheme,
  resolveTheme,
  storeTheme,
  type ThemeMode,
} from "./theme"

export type ThemeController = {
  readonly mode: ThemeMode
  readonly resolved: ResolvedTheme
  readonly setMode: (mode: ThemeMode) => void
}

function browserStorage(): Storage | undefined {
  try {
    return typeof window === "undefined" ? undefined : window.localStorage
  } catch {
    return undefined
  }
}

export function useTheme(): ThemeController {
  const [mode, setStoredMode] = useState<ThemeMode>(() => readStoredTheme(browserStorage()))
  const [prefersDark, setPrefersDark] = useState<boolean>(() =>
    prefersDarkNow(typeof window === "undefined" ? undefined : window),
  )

  useEffect(() => {
    const query = window.matchMedia?.(DARK_QUERY)
    if (query === undefined) return
    const sync = (): void => setPrefersDark(query.matches)
    sync()
    query.addEventListener("change", sync)
    return () => query.removeEventListener("change", sync)
  }, [])

  useEffect(() => {
    applyTheme(mode, document.documentElement)
  }, [mode])

  const setMode = useCallback((next: ThemeMode): void => {
    storeTheme(next, browserStorage())
    setStoredMode(next)
  }, [])

  return { mode, resolved: resolveTheme(mode, prefersDark), setMode }
}
