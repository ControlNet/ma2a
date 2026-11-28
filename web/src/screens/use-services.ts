import { useEffect, useState } from "react"

import { fetchServicesData, type ServicesData } from "../api/services"
import { sessionIsGone } from "../api/web-auth"
import type { ServicesState } from "./services"

/** Operations change without a snapshot revision, so the open Services view polls. */
export const SERVICES_POLL_MS = 3_000

/** Where the view reads its data, and how long it waits after one read before the next. */
export type ServicesPoll = {
  load: (signal?: AbortSignal) => Promise<ServicesData>
  everyMs: number
}

const DEFAULT_POLL: ServicesPoll = { load: fetchServicesData, everyMs: SERVICES_POLL_MS }

/**
 * Polls while `active`. The next read starts only once the last one settled, so a slow read
 * can never land after, and overwrite, a newer one. A read refused because the session is gone
 * stops the polling and reports `onSessionExpired`.
 */
export function useServices(
  active: boolean,
  onSessionExpired: () => void,
  poll: ServicesPoll = DEFAULT_POLL,
): ServicesState {
  const [state, setState] = useState<ServicesState>({ status: "pending" })
  useEffect(() => {
    if (!active) {
      setState({ status: "pending" })
      return
    }
    let stopped = false
    let timer: number | undefined
    const controller = new AbortController()
    const load = (): void => {
      void poll
        .load(controller.signal)
        .then(
          (data) => {
            if (!stopped) setState({ status: "ready", data })
          },
          (error: unknown) => {
            if (stopped) return
            if (sessionIsGone(error)) {
              stopped = true
              controller.abort()
              onSessionExpired()
              return
            }
            setState({ status: "failed" })
          },
        )
        .finally(() => {
          if (!stopped) timer = window.setTimeout(load, poll.everyMs)
        })
    }
    load()
    return () => {
      stopped = true
      controller.abort()
      window.clearTimeout(timer)
    }
  }, [active, onSessionExpired, poll])
  return state
}
