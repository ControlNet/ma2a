# WebUI Security

WebUI is stopped when the daemon starts. It opens a listener only after explicit `ma2a ui start`
and refuses to start until a password is configured. The default binding is `127.0.0.1` with an
OS-selected port. `--host` may select any local IP address or hostname, including wildcard
`0.0.0.0`/`::`; `--port` chooses a fixed port. Remote peers are accepted for non-loopback bindings.
An explicit binding accepts its configured hostname or resolved IP in Host; a wildcard binding
accepts any valid Host at the listener port. Origin must still exactly match `http://<Host>`.

Passwords are established or reset through `ma2a ui init`. The CLI reads the password twice without
terminal echo. The operation creates or replaces the verifier in one transaction and does not
start WebUI. Browser password setup and reset routes do not exist.

The store persists an Argon2id v19 PHC verifier using 19,456 KiB memory, two iterations, one lane, a unique random salt, and a 32-byte output. A password change increments the authentication epoch and revokes all prior sessions.

Login requires exactly one same-origin `Origin`, a bounded body, a request deadline, and a bounded rolling attempt history. When present, `Sec-Fetch-Site` must occur exactly once and equal `same-origin`. Explicitly started HTTP listeners also accept absent Fetch Metadata because browsers may omit it for non-trustworthy HTTP URLs; Origin and CSRF checks still apply. See the [Fetch Metadata specification](https://www.w3.org/TR/fetch-metadata/). Duplicate `Host`, `Origin`, `Sec-Fetch-Site`, session-cookie headers, or `ma2a_session` cookie pairs fail closed. A successful login creates independent 32-byte random bearer and CSRF values. SQLite stores only domain-separated BLAKE3 digests, the authentication epoch, and idle and absolute deadlines.

The session cookie is host-only, `HttpOnly`, and `SameSite=Strict`. Plain HTTP cookies omit `Secure`; an HTTPS deployment must set it. A separate host-only, `SameSite=Strict` CSRF cookie lets the browser restore the per-session CSRF value after a reload without exposing the bearer. Authenticated mutations additionally require that value in `X-CSRF-Token`, an exact same-origin request, and `application/json`. Mutation bodies are parsed as the existing closed local API command contract and forwarded through the private `LocalApiClient`; Web routes contain no separate state mutation logic.

An authenticated request slides its own session's idle deadline without advancing the Runtime revision, because no snapshot projects a session's timestamps. An SSE connection validates its session without extending session deadlines at all. Expiry or revocation closes the stream without emitting a revision, snapshot-derived payload, or other state-bearing recovery frame.

All responses set a restrictive Content Security Policy, `X-Content-Type-Options: nosniff`, `X-Frame-Options: DENY`, and `Referrer-Policy: no-referrer`. API and error responses default to `Cache-Control: no-store`; SPA documents use `no-cache`, and content-hashed `assets/*` use immutable one-year caching. The server emits no permissive CORS headers.
