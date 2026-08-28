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
  { path: ROUTE_PATHS.overview, label: "Overview" },
  { path: ROUTE_PATHS.endpoint, label: "Endpoint" },
  { path: ROUTE_PATHS.spaces, label: "Spaces" },
  { path: ROUTE_PATHS.relays, label: "Relays" },
  { path: ROUTE_PATHS.echo, label: "Echo Test" },
  { path: ROUTE_PATHS.settings, label: "Settings" },
] as const

export function isRoutePath(path: string): path is RoutePath {
  return Object.values(ROUTE_PATHS).some((routePath) => routePath === path)
}
