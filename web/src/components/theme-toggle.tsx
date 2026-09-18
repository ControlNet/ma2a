import type { ReactNode } from "react"

import type { ThemeMode } from "../theme/theme"
import { THEME_MODES } from "../theme/theme"
import { useTheme } from "../theme/use-theme"

const LABELS: Record<ThemeMode, string> = {
  system: "System",
  light: "Light",
  dark: "Dark",
}

export function ThemeToggle(): ReactNode {
  const { mode, resolved, setMode } = useTheme()
  return (
    <fieldset className="segmented">
      <legend className="visually-hidden">Colour theme</legend>
      {THEME_MODES.map((option) => (
        <button
          aria-pressed={mode === option}
          className="segmented__option"
          key={option}
          onClick={() => setMode(option)}
          type="button"
        >
          {LABELS[option]}
        </button>
      ))}
      <span className="visually-hidden" role="status">
        {mode === "system"
          ? `Following the system theme, currently ${resolved}.`
          : `Theme locked to ${resolved}.`}
      </span>
    </fieldset>
  )
}
