# Todo 22 Release Correction

- `release::check` must compose the authoritative `support::check` before deriving the supported
  subset. Comparing cargo-dist only to the currently present supported entries cannot detect a
  silently removed deferred architecture.
- Regression coverage should delete each required target independently. Supported-target omissions
  exercise release/config equality; deferred-target omissions prove the exact matrix contract is
  actually composed.
- Extracted-binary release smoke should authenticate through the real Web contract using owner-only
  request, cookie, and curl-config files. Logs should contain only method, path, and status.
- Offline UI verification should traverse browser-loading references from served HTML, CSS, and
  JavaScript rather than reject every URL-like constant in minified bundles; libraries legitimately
  contain namespace, documentation, and validation strings that are not network fetches.
- A strong negative probe can make a same-length replacement inside the embedded binary, rebuild a
  checksum-valid archive, prove inventory validation still passes, and require runtime smoke to
  reject the external asset reference.
- Evidence claiming recomputable artifact integrity must include the artifact bytes and checksum
  sidecar, not only digest and inventory transcripts.
- Logout smoke must preserve the pre-logout cookie jar and replay it after logout. Reusing the jar
  written by the logout response only proves client-side cookie deletion, not server-side revocation.
- Runtime-reference claims should be bounded to recognized literal browser-loading forms. A dedicated
  scanner with adversarial fixtures keeps the packaged-artifact probe focused while making its exact
  coverage independently reviewable.
- When production rejects both secure and insecure protocol variants, deterministic fixtures should
  contain and assert each emitted scheme independently; a `wss://` case does not prove `ws://` coverage.
