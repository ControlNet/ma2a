# Task 11 Web Authentication

- Schema v2 adds `ui_credentials.auth_epoch` and replaces schema-v1 sessions with bearer digest, CSRF digest, epoch, idle deadline, absolute deadline, and revocation fields. Legacy sessions are discarded during migration because they cannot satisfy the stronger schema.
- Password verifiers are Argon2id v19 PHC strings with `m=19456 KiB`, `t=2`, `p=1`, and 32-byte output. Verification rejects PHC strings whose algorithm, version, or resource parameters differ from this bounded contract.
- Password set/reset is CLI-only. It increments the authentication epoch and revokes all active sessions in one SQLite transaction.
- Bearer and CSRF values each originate from 32 random bytes. Transport uses lowercase hexadecimal; SQLite stores only domain-separated BLAKE3 digests. Secret-bearing result types use redacted `Debug` implementations and zeroizing strings.
- Sessions use a 15-minute sliding idle expiry and an 8-hour absolute expiry. Authentication checks revocation, epoch, both deadlines, and a constant-time digest comparison before sliding idle state.
- The Axum server binds `127.0.0.1` and `[::1]` on one OS-selected port. Accepted peers and Host are fail-closed; mutations also require exactly one same-origin Origin and exactly one `Sec-Fetch-Site: same-origin`. Duplicate security headers or session cookies are rejected.
- Login is body-, time-, session-count-, and rolling-attempt-bounded. Authenticated mutations require the session cookie and per-session `X-CSRF-Token`.
- Security headers are applied to every response, CORS headers are absent, and browser credential setup/reset routes do not exist.
- The app does not open the store or accept `MA2A_STATE_DIR`. CLI credential commands read the password twice without echo and dispatch typed Todo 4 commands through an injected current-user control-client boundary. Until a transport is wired, the installed CLI returns an explicit local-control-unavailable error.
- Migration and all credential/session state decisions run under SQLite immediate transactions. Existing v1 credentials migrate to epoch 1; concurrent capacity-bounded session creation has one durable winner.
- The installed credential CLI now crosses the authenticated owner-private daemon IPC boundary: `RuntimeControlClient` autostarts the daemon, calls `LocalApiClient`, and decodes typed UI results. The daemon owns `CurrentUserRuntime` and executes UI commands with the same request replay protections as other mutations.
