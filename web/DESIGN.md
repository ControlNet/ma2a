# MA2A Runtime Console Design System

## 1. Thesis

The console exists to keep three separations visible, because the Runtime keeps them and a UI that
blurs them is lying:

1. **Signed authorization is not network reachability.** A Space authorizes; a relay only carries
   bytes. A peer that cannot be reached is still fully authorized.
2. **Desired configuration is not observed effective state.** MA2A supplies relay candidates; Iroh
   alone probes, selects a home and upgrades to direct. Observed state is cleared at every restart.
3. **The browser is the least trusted surface.** Passphrases, invitation ticket secrets and private
   key material never enter it. Snapshot uncertainty discards state rather than merging it.

The layout is the argument. The map draws signed lanes and observed peers as two labelled bands
that are never mixed, and the colour system gives each side its own palette.

Design dials: variance 3, motion 2, density 6.

## 2. Colour

One token set, two modes, defined once in `src/styles/tokens.css`. `:root` carries light,
`@media (prefers-color-scheme: dark) :root:not([data-theme="light"])` carries automatic dark, and
`:root[data-theme="dark"]` carries the explicit choice. `src/styles/tokens.test.ts` asserts the two
dark blocks are identical token for token.

### Two groups that never mix

| Group | Tokens | Means |
| --- | --- | --- |
| Interaction | `--accent`, `--accent-soft`, `--accent-contrast` | something you can act on, this Endpoint, focus |
| Observation | `--observed-direct`, `--observed-relay`, `--observed-failed`, `--observed-none` (+ `-soft`) | what Iroh reported |

A signed fact is never painted with an observation tone. Lanes, the connector to the self node and
every membership surface use neutral `--line` / `--panel` tokens, because membership is not a
status.

### Values

| Token | Light | Dark | Use |
| --- | --- | --- | --- |
| `--bg` | `#f7f9fc` | `#0e1116` | page ground |
| `--panel` | `#ffffff` | `#161a21` | lane, card, rail, inspector |
| `--panel-2` | `#f1f5fa` | `#1c222b` | chip, input, inset |
| `--line` | `#e1e7ef` | `#242b35` | hairline between surfaces |
| `--track` | `#dce3ec` | `#2a313c` | chart track behind a value |
| `--control` | `#798fad` | `#5c6d85` | the visible boundary of a control |
| `--text` | `#0d1420` | `#e6eaf0` | primary copy and identifiers |
| `--text-muted` | `#4e5a6b` | `#9aa7b8` | secondary copy |
| `--text-dim` | `#646f7f` | `#7c8a9b` | eyebrows, chart ticks, captions |
| `--accent` | `#2e62c8` | `#6d9bf2` | interaction |
| `--observed-direct` | `#0a7e54` | `#3ddc97` | direct path observed |
| `--observed-relay` | `#976409` | `#f0b429` | relay path observed |
| `--observed-failed` | `#c0392b` | `#ff6b6b` | failed, revoked, offline |
| `--observed-none` | `#646f7f` | `#7c8a9b` | no observation retained |

Every value above is derived, not chosen by eye. `tokens.test.ts` enforces, in **both** modes:
every readable token at 4.5:1 on `--bg`, `--panel` and `--panel-2`; every `-soft` fill at 4.5:1
against its own foreground; `--accent-contrast` at 4.5:1 on `--accent`; and `--control` at 3:1 on
every surface, because it is the only visual boundary some controls have.

## 3. Typography

Both families are self-hosted, latin subset only, declared in `src/styles/fonts.css` and vendored
through `@fontsource-variable/*`. `src/styles/fonts.test.ts` fails if any stylesheet ever references
a remote origin, so the embedded binary stays offline by construction.

- UI: `Space Grotesk Variable`, falling back to `ui-sans-serif, system-ui`
- Identifiers: `JetBrains Mono Variable`, falling back to `ui-monospace`

| Level | Size | Weight | Usage |
| --- | --- | --- | --- |
| Screen title | `1.375rem` | 700 | one `h1` per route |
| Inspector title | `1.0625rem` | 600 | the selected object |
| Body | `0.8125rem` | 400 | explanatory copy |
| Label | `0.75rem` | 600 | field labels, meter labels |
| Eyebrow | `0.625rem` | 600, `0.12em`, uppercase | card and section kickers |
| Identifier | `0.8125rem` mono | 500 | Endpoint and Space IDs |

Identifiers wrap with `overflow-wrap: anywhere` and get a copy action rather than being shrunk.

## 4. Layout

- `.shell` is a fixed `100dvb` grid: top strip, then rail plus scrolling main plus optional
  inspector. The main region is the only vertical scroll owner on the desktop.
- Rail `72px`, inspector `340px`, both tokens.
- Below `74rem` the inspector moves under the content and the page becomes one scroll owner.
- Below `40rem` the rail becomes a fixed bottom tab bar. All six destinations stay visible at every
  width, so nothing is ever scrolled into view and the sequential focus origin never moves.
- Repeated groups use `repeat(auto-fit, minmax(min(19rem, 100%), 1fr))`.

## 5. Components

`src/viz/` holds every chart. Each takes semantic props and reads colour from tokens, so light and
dark are one implementation.

| Component | Draws | Backed by |
| --- | --- | --- |
| `LaneMap` | signed lanes and observed peers as two labelled bands | `spaces`, `connections`, `control_sync` |
| `StateMachine` | the four named `RelayReachability` states | derived in `runtime-view.ts` |
| `MeterList` | any documented bound or countdown, value always spelled out | `connections`, `spaces`, payload length |
| `StackBar` | peer path distribution | `connections` |
| `Donut` / `Ring` | bounded ratios | `recent_echo_summary`, `ui_auth` |
| `TrustChain` | where each secret may exist | documented boundaries, static |

Rules that hold across all of them:

- Status is never colour alone. Every pill, cell and segment carries its own word, and
  `visually-hidden` text names what a mark means.
- Dashed means unobserved, not broken. A peer with no observation keeps full presence.
- A table is a `<table>`. Coverage and trust grids are read by row or column with real headers.
- Nothing renders from a fixture or from cached browser state. `PendingSnapshot` is deliberately
  empty rather than a plausible skeleton of data.

## 6. Motion

`--motion-micro` 120ms for hover, press and selection; `--motion-standard` 180ms for disclosure.
Only colour, opacity and transform animate. `prefers-reduced-motion: reduce` removes the pending
pulse and every transition.

## 7. Accessibility

WCAG 2.2 AA, with zero serious or critical axe findings on all eight routes asserted in
`src/app.test.tsx`. Targets are at least `--target-min` (44px). Focus is a 3px `--accent` outline at
3px offset and is never colour alone. Layout is usable at 375px, 768px, 1280px and 200% zoom.

The theme preference is the only thing this console writes to browser storage. Reads and writes are
wrapped, because blocked storage must degrade to `system` rather than throw.

## 8. What the snapshot now carries

Every visualisation in section 5 is backed by a published field. Where a field did
not exist, the local API was extended rather than the picture faked; the schema is
pinned by SHA-256 and validated from both Rust and TypeScript, so each addition
moved the Rust model, the schema, its hash, both generated models, the runtime zod
parser and the docs together.

| Drawn | Backed by |
| --- | --- |
| Signed member set inside each Space | `snapshot_space.members`, with label and capability grants |
| Generation and chain hash | `snapshot_space.generation`, `.chain_hash`, `.revoked_count` |
| Relay coverage grid | `relay_candidate.covered_space_ids`, with `eligible` as the verdict |
| Retained observation track | `connection.observations`, at most eight per peer |
| Control round history | `runtime_snapshot.control_rounds`, at most sixteen, memory only |
| Echo round-trip time | `echo_reply.duration_ms`, saturated at 10,000 |

One thing is still deliberately absent, and the UI says so rather than guessing:
which Space authorized a given request. `authorized_via` is on the schema's own
privacy exclusion list beside private keys and invite secrets.

A manifest chain **ladder** is still not drawn. The snapshot publishes the chain
head, not the history behind it, and a ladder of one rung would be a decoration.

## 9. Accepted debt

| Item | Why accepted | Exit |
| --- | --- | --- |
| Tailwind is imported but unused | the preflight reset is still wanted; utilities are not | drop the import if the reset is ever inlined |
