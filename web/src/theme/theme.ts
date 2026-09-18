export type ThemeMode = "system" | "light" | "dark"
export type ResolvedTheme = "light" | "dark"

export const THEME_MODES = ["system", "light", "dark"] as const
export const THEME_STORAGE_KEY = "ma2a-theme"
export const DARK_QUERY = "(prefers-color-scheme: dark)"

export function isThemeMode(value: unknown): value is ThemeMode {
  return THEME_MODES.some((mode) => mode === value)
}

/** Reads the stored preference. Blocked or cleared storage means "system". */
export function readStoredTheme(storage: Storage | undefined): ThemeMode {
  try {
    const stored = storage?.getItem(THEME_STORAGE_KEY)
    return isThemeMode(stored) ? stored : "system"
  } catch {
    return "system"
  }
}

/** Persists the preference. A theme is not secret material, so storage is fine. */
export function storeTheme(mode: ThemeMode, storage: Storage | undefined): void {
  try {
    if (mode === "system") {
      storage?.removeItem(THEME_STORAGE_KEY)
      return
    }
    storage?.setItem(THEME_STORAGE_KEY, mode)
  } catch {
    return
  }
}

export function resolveTheme(mode: ThemeMode, prefersDark: boolean): ResolvedTheme {
  if (mode === "system") return prefersDark ? "dark" : "light"
  return mode
}

/**
 * "system" removes the attribute so the media query in tokens.css decides.
 * An explicit mode sets it so the same file's attribute rules win instead.
 */
export function applyTheme(mode: ThemeMode, root: HTMLElement | undefined): void {
  if (root === undefined) return
  if (mode === "system") {
    root.removeAttribute("data-theme")
    return
  }
  root.setAttribute("data-theme", mode)
}

export function prefersDarkNow(view: Window | undefined): boolean {
  try {
    return view?.matchMedia?.(DARK_QUERY).matches === true
  } catch {
    return false
  }
}
