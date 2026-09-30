import { useEffect, useState } from "react"

import { fetchServicesData } from "../api/services"
import type { ServicesState } from "./services"

/** Operations change without a snapshot revision, so the open Services view polls. */
export const SERVICES_POLL_MS = 3_000

export function useServices(active: boolean): ServicesState {
  const [state, setState] = useState<ServicesState>({ status: "pending" })
  useEffect(() => {
    if (!active) {
      setState({ status: "pending" })
      return
    }
    let stopped = false
    const load = (): void => {
      void fetchServicesData().then(
        (data) => {
          if (!stopped) setState({ status: "ready", data })
        },
        () => {
          if (!stopped) setState({ status: "failed" })
        },
      )
    }
    load()
    const timer = window.setInterval(load, SERVICES_POLL_MS)
    return () => {
      stopped = true
      window.clearInterval(timer)
    }
  }, [active])
  return state
}
