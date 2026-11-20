import { useEffect, useState } from "react"

import { fetchServicesData, type ServicesData } from "../api/services"
import type { ServicesState } from "./services"

/** Operations change without a snapshot revision, so the open Services view polls. */
export const SERVICES_POLL_MS = 3_000

/** Where the view reads its data, and how long it waits after one read before the next. */
export type ServicesPoll = { load: () => Promise<ServicesData>; everyMs: number }

const DEFAULT_POLL: ServicesPoll = { load: fetchServicesData, everyMs: SERVICES_POLL_MS }

/**
 * Polls while `active`. The next read starts only once the last one settled, so a slow read
 * can never land after, and overwrite, a newer one.
 */
export function useServices(active: boolean, poll: ServicesPoll = DEFAULT_POLL): ServicesState {
  const [state, setState] = useState<ServicesState>({ status: "pending" })
  useEffect(() => {
    if (!active) {
      setState({ status: "pending" })
      return
    }
    let stopped = false
    let timer: number | undefined
    const load = (): void => {
      void poll
        .load()
        .then(
          (data) => {
            if (!stopped) setState({ status: "ready", data })
          },
          () => {
            if (!stopped) setState({ status: "failed" })
          },
        )
        .finally(() => {
          if (!stopped) timer = window.setTimeout(load, poll.everyMs)
        })
    }
    load()
    return () => {
      stopped = true
      window.clearTimeout(timer)
    }
  }, [active, poll])
  return state
}
