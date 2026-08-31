# MA2A Runtime Console Design System

## 0. Research Log

- Embedded references: shortlisted Linear, Vercel, and Supabase for precise developer-tool
  structure; picked the operational taste rules plus Linear's compact hierarchy and luminance
  discipline, translated to a light system rather than copied as a dark brand surface.
- Lazyweb: ran 3 desktop queries and viewed 6 shipped screens from Uptrends, Atatus, Appwrite,
  CommandBar, Netdata, and AppSignal. Adopted persistent domain navigation, a divided metric rail,
  retained structure for empty data, endpoint-state legends, and wide evidence plus narrow detail
  layouts. No screenshots or brand assets ship with MA2A.
- Imagen drafts: generated one session triptych containing 3 desktop directions. The image tool did
  not expose a filesystem path. Chose direction A, "light technical paper," with direction B's
  evidence/detail composition. Rejected the dark direction because this task calls for a restrained
  light operational shell.
- UI/UX database: retained the high-contrast, 44px target, visible-focus, and reduced-motion
  guidance. Rejected its healthcare classification, remote Google Fonts, orange CTA, shadowed card,
  and marketing-page recommendations because they conflict with the product and offline build.
- Interaction reference: consulted beui.dev shared-layout navigation and loader mechanisms. MA2A
  uses their state-continuity and reduced-motion ideas with CSS only; no motion dependency is added.

## 1. Atmosphere & Identity

MA2A is a quiet local command surface for an operator who needs truthful runtime state more than
decoration. It should feel like a precise technical document that happens to be live: cool paper,
ink, hairline rules, and small areas of cobalt reserved for actions and focus. The signature is the
revision line, a continuous horizontal state rail that keeps identity, Spaces, relay posture,
reachability, and Echo status readable without a grid of cards.

Primary persona: a technical operator checking whether one local Runtime is correctly connected,
authorized, and able to Echo a target. Stress personas include keyboard-only users, users at 200%
zoom, users with reduced motion, users with color-vision differences, and operators diagnosing an
offline or uncertain snapshot under time pressure.

Design dials: variance 3, motion 2, density 7. Hierarchy and alignment carry the interface; motion
only confirms navigation, disclosure, and state replacement.

## 2. Color

### Palette

| Role | Token | Value | Usage |
| --- | --- | --- | --- |
| Canvas | `--surface-canvas` | `#f4f6f8` | Application background |
| Primary surface | `--surface-primary` | `#fbfcfd` | Main workspace and fields |
| Secondary surface | `--surface-secondary` | `#eef2f5` | Sidebar, selected rows, code wells |
| Elevated surface | `--surface-elevated` | `#ffffff` | Menus and focused overlays only |
| Ink | `--text-primary` | `#17202a` | Headings, values, body text |
| Secondary ink | `--text-secondary` | `#465565` | Descriptions and metadata |
| Muted ink | `--text-muted` | `#5f6d7c` | Disabled and tertiary metadata |
| Rule | `--border-default` | `#cfd7df` | Structural dividers and fields |
| Subtle rule | `--border-subtle` | `#e2e7ec` | Row separators |
| Accent | `--accent-primary` | `#185fa7` | Links, selected navigation, primary actions |
| Accent hover | `--accent-hover` | `#124c86` | Hover and active accent state |
| Focus | `--focus-ring` | `#0b63ce` | Keyboard-visible focus outline |
| Success | `--status-success` | `#19733b` | Confirmed healthy state |
| Warning | `--status-warning` | `#8a5a00` | Degraded or uncertain state |
| Warning surface | `--status-warning-surface` | `#fff7df` | Uncertain snapshot banner |
| Error | `--status-error` | `#b42318` | Failed or offline state |
| Error surface | `--status-error-surface` | `#fff0ee` | Error messaging |
| Info surface | `--status-info-surface` | `#eaf3fc` | Selected and informational state |

### Rules

- The accent identifies interactivity, never decoration.
- Status always combines color with text and, where compact, a shape or icon.
- Primary reading contrast targets WCAG 2.2 AA, with 4.5:1 for normal text and 3:1 for large text
  and interface boundaries.
- No gradients, translucent glass, remote assets, or theme inversion appear in this task.

## 3. Typography

### Font Stack

- Primary: `ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif`
- Mono: `ui-monospace, "SFMono-Regular", Consolas, "Liberation Mono", monospace`

Both stacks are local system fonts so the embedded binary remains fully offline.

### Scale

| Level | Size | Weight | Line height | Tracking | Usage |
| --- | --- | --- | --- | --- | --- |
| Page title | `2rem` | 650 | 1.15 | `-0.025em` | Screen title |
| Section title | `1.25rem` | 650 | 1.3 | `-0.015em` | Major section heading |
| Subheading | `1rem` | 650 | 1.4 | `-0.01em` | Group title |
| Body | `1rem` | 400 | 1.55 | normal | Default content and forms |
| Body small | `0.875rem` | 400 | 1.45 | normal | Navigation and metadata |
| Label | `0.75rem` | 650 | 1.35 | `0.04em` | Compact field labels |
| Mono | `0.8125rem` | 500 | 1.5 | normal | Endpoint, Space, relay, and revision values |

Body text never drops below 14px. Long identifiers use monospace, `overflow-wrap: anywhere`, and a
copy action rather than tiny type.

## 4. Spacing & Layout

### Spacing

All intent spacing derives from a 4px base.

| Token | Value | Usage |
| --- | --- | --- |
| `--space-1` | `0.25rem` | Tight icon or status gaps |
| `--space-2` | `0.5rem` | Inline clusters and row gaps |
| `--space-3` | `0.75rem` | Compact control padding |
| `--space-4` | `1rem` | Standard control and row padding |
| `--space-5` | `1.25rem` | Dense section inset |
| `--space-6` | `1.5rem` | Standard section gap |
| `--space-8` | `2rem` | Major content separation |
| `--space-10` | `2.5rem` | Page title to first section |

### Shell

- The application is a `fixed-sidenav-shell` bounded by `100dvb`.
- Desktop: 15rem fixed navigation rail and a fluid main column. The main column is the only vertical
  scroll owner and has `min-block-size: 0`.
- Below 768px: navigation becomes a horizontally scrollable landmark row below the brand header;
  the main region becomes one readable column with no primary-content horizontal scroll.
- Main content is capped at 90rem with fluid inline padding via `clamp()`.
- Repeated data groups use an overflow-safe intrinsic grid:
  `repeat(auto-fit, minmax(min(16rem, 100%), 1fr))`.
- The state rail is a continuous grid separated by hairline dividers. It stacks into labeled rows on
  narrow screens instead of becoming individual cards.
- Detail screens use a wide evidence region plus a narrow status rail at desktop and one column below
  1024px.

## 5. Components

### Application Shell

- **Structure**: skip link, header/brand, primary `nav`, scroll-owning `main`, runtime footer status.
- **States**: active route, hover, focus-visible, offline, snapshot uncertain.
- **Accessibility**: one `h1`, named navigation, current route via `aria-current`, 44px targets.
- **Motion**: selected navigation changes background and inset rule over 120ms; instant under reduced
  motion.

### Page Header

- **Structure**: title and plain-language description, optional compact action cluster.
- **Variants**: standard, centered authentication, setup notice.
- **Layout**: stack on narrow widths; actions wrap without label truncation.

### State Rail

- **Structure**: definition-list cells in one continuous region with labels and values.
- **Variants**: loading skeleton, authoritative, uncertain, empty.
- **States**: healthy, degraded, unknown, offline. Each state includes visible text.
- **Accessibility**: semantic `dl`; live uncertainty announcement is outside the rail.

### Status Banner

- **Structure**: status icon, concise heading, recovery explanation, optional action.
- **Variants**: info, warning/uncertain, error/offline.
- **States**: default, focus-visible action, loading recovery.
- **Accessibility**: `role="status"` for recovery, `role="alert"` for blocking errors; never color-only.

### Section

- **Structure**: heading, optional description/action, content separated by whitespace or a top rule.
- **Variants**: standard, divided list, split evidence/detail.
- **Rules**: sections are not generic cards; elevation is reserved for overlays.

### Data List

- **Structure**: semantic list or table depending on comparison needs; stable headings remain visible
  in empty states.
- **Variants**: zero, one, many, loading, error.
- **Accessibility**: table captions where applicable, row actions named with the target identity.

### Field and Button

- **Structure**: visible label above control, helper or error text below, buttons use native elements.
- **Variants**: primary, secondary, quiet, destructive; text, password, and target Endpoint field.
- **States**: default, hover, active, focus-visible, disabled, loading, error.
- **Accessibility**: no placeholder-as-label; minimum 44px control height; errors are programmatically
  associated.
- **Motion**: active press translates by 1px only when reduced motion is not requested.

### Empty State

- **Structure**: retained section heading and column structure, short factual message, one next step
  when available.
- **Variants**: no Spaces, no relays, no Echo history, setup required.
- **Rules**: no decorative illustration, fake data, or invented counts.

### Primitive Showcase

The `/showcase` development route exercises shell navigation, banners, state rail, buttons, fields,
data lists, and empty/loading/error states. It is excluded from the production route list and exists
only behind `import.meta.env.DEV`.

## 6. Motion & Interaction

| Token | Duration | Easing | Usage |
| --- | --- | --- | --- |
| `--motion-micro` | `120ms` | `ease-out` | Hover, press, focus-adjacent tint |
| `--motion-standard` | `180ms` | `cubic-bezier(0.16, 1, 0.3, 1)` | Navigation and disclosure state |

- Only `transform`, `opacity`, and color/filter transitions animate.
- Route changes update content immediately; no decorative page entrance.
- Loading state uses a low-contrast opacity pulse shaped like the final content.
- State replacement preserves layout so resnapshot recovery does not cause a large visual jump.
- `prefers-reduced-motion: reduce` removes transforms, animation, and non-essential transitions.

## 7. Depth & Surface

Strategy: tonal shift plus hairline rules.

- Canvas, navigation, workspace, and selected rows differ by small luminance steps.
- Hairline dividers express structure in rails, tables, and forms.
- Shadows are allowed only for menus or dialogs that must appear above the shell.
- Standard sections never float as rounded cards. Controls use a 6px radius; large regions use no
  radius unless they are an overlay.

## 8. Accessibility Constraints & Accepted Debt

### Constraints

- Target WCAG 2.2 AA with zero serious or critical axe findings on every route.
- Every route is reachable by keyboard from the skip link and primary navigation.
- Focus is always visible and never indicated by color alone.
- Status changes use text, semantic live regions, and stable placement.
- Layout remains usable at 375px, 768px, 1280px, and 200% zoom.
- Authentication never stores passphrases, bearer tokens, or verifier material in browser storage.
- Setup-required mode reveals instructions only and no management state or browser reset form.
- Snapshot uncertainty is explicit; stale incrementals are not rendered as authoritative state.

### Accepted Debt

| Item | Location | Why accepted | Owner / Exit |
| --- | --- | --- | --- |
| None | N/A | No design debt accepted for this task | N/A |
