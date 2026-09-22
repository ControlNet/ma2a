import ky from "ky"
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
    await http.post(new URL("/api/v1/web/auth/logout", window.location.origin), {
      headers: { "x-csrf-token": session.csrfToken },
    })
  } catch (error) {
    throw new WebAuthenticationError(error)
  }
}
