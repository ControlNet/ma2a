# Loopback Web Security

MA2A's Phase 1 Web surface binds to an operating-system-selected loopback port and rejects accepted peers that are not loopback addresses. Requests must use a literal `127.0.0.1:<port>` or `[::1]:<port>` Host value.

Passwords can only be established or reset through `ma2a init` and `ma2a ui password set|reset`. The CLI reads the password twice without terminal echo. Browser password setup and reset routes do not exist.

The store persists an Argon2id v19 PHC verifier using 19,456 KiB memory, two iterations, one lane, a unique random salt, and a 32-byte output. A password change increments the authentication epoch and revokes all prior sessions.

Login requires exactly one same-origin `Origin`, exactly one `Sec-Fetch-Site: same-origin`, a bounded body, a request deadline, and a bounded rolling attempt history. Duplicate `Host`, `Origin`, `Sec-Fetch-Site`, session-cookie headers, or `ma2a_session` cookie pairs fail closed. A successful login creates independent 32-byte random bearer and CSRF values. SQLite stores only domain-separated BLAKE3 digests, the authentication epoch, and idle and absolute deadlines.

The session cookie is host-only, `HttpOnly`, and `SameSite=Strict`. Plain HTTP loopback cookies omit `Secure`; an HTTPS deployment must set it. Authenticated mutations additionally require the per-session CSRF value in `X-CSRF-Token`.

All responses set a restrictive Content Security Policy, `X-Content-Type-Options: nosniff`, `X-Frame-Options: DENY`, `Referrer-Policy: no-referrer`, and `Cache-Control: no-store`. The server emits no permissive CORS headers.
