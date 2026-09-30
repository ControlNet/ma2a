import type { ServicesData } from "../api/services"

/** Test-only operations data; Space IDs match the Space fixtures so names resolve. */
export const SERVICES_FIXTURE: ServicesData = {
  exposures: [
    {
      space_id: "test-space-laboratory",
      kind: "files_root",
      root: "docs",
      path: "/srv/docs",
      access: "read_only",
      host: null,
      port_start: null,
      port_end: null,
    },
    {
      space_id: "test-space-laboratory",
      kind: "proxy",
      root: null,
      path: null,
      access: null,
      host: "127.0.0.1",
      port_start: 8000,
      port_end: 8080,
    },
    {
      space_id: "ab".repeat(32),
      kind: "exec",
      root: null,
      path: null,
      access: null,
      host: null,
      port_start: null,
      port_end: null,
    },
  ],
  operations: {
    active_count: 1,
    active: [
      {
        request_id: "01".repeat(16),
        peer_endpoint_id: "44".repeat(32),
        service: "shell",
        operation: "shell",
        direction: "inbound",
        elapsed_ms: 65_000,
      },
    ],
    recent_count: 300,
    recent: [
      {
        request_id: "02".repeat(16),
        peer_endpoint_id: "44".repeat(32),
        service: "transfer",
        operation: "download",
        direction: "inbound",
        result: "completed",
        error: null,
        bytes_in: 0,
        bytes_out: 3 * 1024 * 1024,
        duration_ms: 1_500,
      },
      {
        request_id: "03".repeat(16),
        peer_endpoint_id: "55".repeat(32),
        service: "exec",
        operation: "exec",
        direction: "outbound",
        result: "rejected",
        error: "unauthorized",
        bytes_in: 12,
        bytes_out: 0,
        duration_ms: 4,
      },
    ],
  },
}
