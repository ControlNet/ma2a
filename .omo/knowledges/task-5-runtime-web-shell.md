# Task 5 Runtime Web Shell

- The Todo 5 frontend is intentionally fixture-free at runtime. Static typed fixtures live only in
  `web/src/test/fixtures.ts`; production routes render stable loading structure until a later task
  wires the state boundary to the Runtime.
- The API boundary uses Ky with same-origin credentials and Zod 4 schemas. SSE uses
  `withCredentials: true`; API paths must begin with one slash and cannot be protocol-relative.
- Revision continuity is authoritative only after a snapshot. Disconnect, duplicate/out-of-order
  revision, or a gap enters an uncertain state, discards incrementals, and permits one bounded
  resnapshot before accepting the replacement snapshot's next revision.
- Native `bun test --coverage` requires `web/bunfig.toml` to preload the JSDOM globals from
  `web/src/test/setup.ts`; Vitest continues to use its configured JSDOM environment independently.
- The design system is a light technical-paper operational shell with system fonts, cool-blue focus
  and action color, tonal surfaces, hairline dividers, no generic card grid, and reduced motion.
- Verification command: `cd web && bunx biome check . && bunx tsc --noEmit && bun test --coverage && bun run build`.
