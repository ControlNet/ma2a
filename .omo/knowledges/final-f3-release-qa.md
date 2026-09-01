# Final F3 Release QA

- Final reviewed HEAD: `9afeb98b2c5c3e72d996985fc6fbe9544b5242b9`.
- Final native Linux x86_64 archive SHA-256:
  `2635fe38b6ddd528eac5bedf766d3a0732d36e08ae097748a9309ece31f2245d`.
- The prior F3 Echo CLI rejection is resolved: Endpoint plus text/stdin and optional JSON combinations
  parse without usage errors in the extracted archive.
- The tracked release smoke now uses canonical `ui open` and passes directly against the final archive.
- Current HEAD passes the exact 46-case `ma2a-app` E2E target, including real-Iroh Echo and relay flows.
- Browser QA confirms authenticated routes, logout, revoke-all, strict cookies, empty Web Storage,
  responsive layout, deliberate offline state, and restart persistence.
- Local evidence supports native Linux x86_64 only; macOS, Windows, ARM64, and OIDC remain unclaimed.
