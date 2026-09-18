import type { IconName } from "./components/icons"

export const ROUTE_PATHS = {
  login: "/login",
  setup: "/setup",
  overview: "/",
  endpoint: "/endpoint",
  spaces: "/spaces",
  relays: "/relays",
  echo: "/echo",
  settings: "/settings",
} as const

export type RoutePath = (typeof ROUTE_PATHS)[keyof typeof ROUTE_PATHS]

export const RUNTIME_ROUTES = [
  { path: ROUTE_PATHS.overview, label: "Map", icon: "map" },
  { path: ROUTE_PATHS.spaces, label: "Spaces", icon: "spaces" },
  { path: ROUTE_PATHS.endpoint, label: "Peers", icon: "peers" },
  { path: ROUTE_PATHS.relays, label: "Relays", icon: "relays" },
  { path: ROUTE_PATHS.echo, label: "Echo", icon: "echo" },
  { path: ROUTE_PATHS.settings, label: "Settings", icon: "settings" },
] as const satisfies readonly {
  readonly path: RoutePath
  readonly label: string
  readonly icon: IconName
}[]

export type RuntimeRoutePath = (typeof RUNTIME_ROUTES)[number]["path"]

export function isRoutePath(path: string): path is RoutePath {
  return Object.values(ROUTE_PATHS).some((routePath) => routePath === path)
}
