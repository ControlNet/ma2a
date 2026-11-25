import { afterEach, expect, test, vi } from "vitest"

import {
  createRuntimeMutationClient,
  mutationFailureText,
  mutationOutcomeUnknown,
} from "./mutations"

afterEach(() => {
  vi.restoreAllMocks()
})

// Client-side ID validation fails before any HTTP request leaves the browser.
test("client-side validation errors never report an unknown outcome or call fetch", async () => {
  const fetchSpy = vi.spyOn(globalThis, "fetch")
  const client = createRuntimeMutationClient({ csrfToken: () => "cd".repeat(32) })
  const attempts = [
    client.configurePrivateRelay({
      mode: "external_termination",
      listen: "127.0.0.1:443",
      publicUrl: "https://relay.example",
      servedSpaceIds: ["not-a-hex-id"],
      certificatePath: null,
      privateKeyPath: null,
    }),
    client.createInvitation("not-a-space-id", 300_000, "/tmp/invite"),
    client.revokeEndpoint("ab".repeat(32), "not-an-endpoint-id"),
    client.triggerSync("not-an-endpoint-id"),
  ]

  for (const attempt of attempts) {
    const error: unknown = await attempt.catch((caught) => caught)
    expect(mutationOutcomeUnknown(error)).toBe(false)
    expect(mutationFailureText(error, "No answer came back.", "Rejected.")).toMatch(
      /64 lowercase hexadecimal/,
    )
  }

  expect(fetchSpy).not.toHaveBeenCalled()
})
