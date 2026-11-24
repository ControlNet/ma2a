import ky, { HTTPError } from "ky"
import { z } from "zod"

const LoginResponseSchema = z
  .strictObject({
    csrf_token: z.string().regex(/^[0-9a-f]{64}$/),
  })
  .readonly()

const CsrfTokenSchema = z.string().regex(/^[0-9a-f]{64}$/)
const CSRF_COOKIE = "ma2a_csrf"

const http = ky.create({
  credentials: "same-origin",
  retry: 0,
  timeout: 5_000,
})

export class WebAuthenticationError extends Error {
  readonly name = "WebAuthenticationError"

  constructor(readonly cause: unknown) {
    super("The Runtime did not accept this sign-in attempt")
  }
}

export type WebSession = {
  readonly csrfToken: string
}

export function currentWebSession(): WebSession | undefined {
  const tokens = document.cookie
    .split(";")
    .map((part) => part.trim().split("=", 2))
    .filter(([name]) => name === CSRF_COOKIE)
    .map(([, value]) => CsrfTokenSchema.safeParse(value))
  if (tokens.length !== 1) {
    return undefined
  }
  const token = tokens[0]
  return token?.success === true ? { csrfToken: token.data } : undefined
}

export async function loginAndTouchSession(password: string): Promise<WebSession> {
  try {
    const payload: unknown = await http
      .post(new URL("/api/v1/web/auth/login", window.location.origin), {
        json: { password },
      })
      .json()
    const session = LoginResponseSchema.parse(payload)
    await http.post(new URL("/api/v1/web/session/touch", window.location.origin), {
      headers: { "x-csrf-token": session.csrf_token },
    })
    return { csrfToken: session.csrf_token }
  } catch (error) {
    throw new WebAuthenticationError(error)
  }
}

export async function logoutSession(session: WebSession): Promise<void> {
  try {
    await postLogout(session.csrfToken)
  } catch (error) {
    // Another tab may have signed in again since this page loaded, replacing the session cookie
    // with one whose CSRF token this page never saw. The browser's current session is still
    // valid then, so log it out with the token its cookie carries instead of reporting success.
    const current = currentWebSession()
    if (
      error instanceof HTTPError &&
      error.response.status === 403 &&
      current !== undefined &&
      current.csrfToken !== session.csrfToken
    ) {
      try {
        await postLogout(current.csrfToken)
        return
      } catch (retry) {
        throw new WebAuthenticationError(retry)
      }
    }
    throw new WebAuthenticationError(error)
  }
}

/**
 * Whether a failed logout found the session already gone, so the page may sign out.
 *
 * The Runtime answers 401 only for a session that no longer exists; any other refusal, 403
 * for a stale CSRF token included, leaves the browser's session live.
 */
export function sessionIsGone(error: unknown): boolean {
  const cause = error instanceof WebAuthenticationError ? error.cause : error
  return cause instanceof HTTPError && cause.response.status === 401
}

async function postLogout(csrfToken: string): Promise<void> {
  await http.post(new URL("/api/v1/web/auth/logout", window.location.origin), {
    headers: { "x-csrf-token": csrfToken },
  })
}
