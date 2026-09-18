import { vi } from "vitest"

import type { RuntimeActions } from "../runtime-actions"

export function runtimeActions(): RuntimeActions {
  return {
    createSpace: vi.fn(async () => undefined),
    createInvitation: vi.fn(async () => undefined),
    revokeEndpoint: vi.fn(async () => undefined),
    triggerSync: vi.fn(async () => undefined),
    configurePrivateRelay: vi.fn(async () => ({
      configured: true,
      mode: "external_termination" as const,
      host: "127.0.0.1",
      port: 443,
      online: true,
    })),
    configurePublicRelay: vi.fn(async () => ({ configured: true, url: null, online: false })),
    echo: vi.fn(async () => ({
      target_endpoint_id: "11".repeat(32),
      payload: "ok",
      duration_ms: 34,
    })),
    revokeSessions: vi.fn(async () => undefined),
  }
}
