# Todo 5 adversarial frontend QA

- Browser-rendered axe checks are necessary even when jsdom axe assertions pass. jsdom did not expose the real CSS contrast failure for `--text-muted` on the sidebar surface.
- Put reduced-motion overrides after the declarations they neutralize, or give the overrides equivalent cascade precedence. An early media rule in `tokens.css` was overridden by later animation and transition declarations.
- Verify skip navigation from a fresh real-browser document with normal Tab traversal. Programmatically focusing the skip link proves its click handler, but does not prove the link is actually first in sequential focus order.
- Keep repository policy checks in the frontend acceptance gate. A green formatter, type check, test suite, and production build can still fail the workspace LOC policy.
- A horizontally off-screen current mobile route can remain discoverable without automatic scrolling when a persistent visible current-route label and explicit scroll hint are present; verify both the DOM semantics and the narrow screenshot.
- Test the actual overflow owner with keyboard input. Focusing the named `main` and observing its `scrollTop` change after PageDown proves more than checking `overflow-y: auto` in source.
- Reduced-motion acceptance should record Chromium computed styles for each affected class, including both animation and transition properties, after emulating `prefers-reduced-motion: reduce`.
