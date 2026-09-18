import type { ReactNode } from "react"

const PATHS = {
  map: "M3 6l6-3 6 3 6-3v15l-6 3-6-3-6 3z",
  spaces: "M4 5h16M4 12h16M4 19h16",
  peers: "M4 12h4l3-7 3 14 3-7h4",
  relays: "M12 4v16M4 8h16M4 16h16",
  echo: "M5 12a7 7 0 0 1 14 0M8 12a4 4 0 0 1 8 0M12 12v6",
  settings: "M12 8a4 4 0 100 8 4 4 0 000-8zM12 2v3M12 19v3M2 12h3M19 12h3",
} as const

export type IconName = keyof typeof PATHS

export function Icon({ name }: { readonly name: IconName }): ReactNode {
  return (
    <svg aria-hidden="true" fill="none" height="20" viewBox="0 0 24 24" width="20">
      <path
        d={PATHS[name]}
        stroke="currentColor"
        strokeLinecap="round"
        strokeLinejoin="round"
        strokeWidth="1.6"
      />
    </svg>
  )
}
