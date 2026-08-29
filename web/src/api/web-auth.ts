import ky from "ky"
import { z } from "zod"

const LoginResponseSchema = z
  .strictObject({
    csrf_token: z.string().regex(/^[0-9a-f]{64}$/),
  })
  .readonly()

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

export async function loginAndTouchSession(passphrase: string): Promise<void> {
  try {
    const payload: unknown = await http
      .post(new URL("/api/v1/web/auth/login", window.location.origin), {
        json: { password: passphrase },
      })
      .json()
    const session = LoginResponseSchema.parse(payload)
    await http.post(new URL("/api/v1/web/session/touch", window.location.origin), {
      headers: { "x-csrf-token": session.csrf_token },
    })
  } catch (error) {
    throw new WebAuthenticationError(error)
  }
}
