# Task 13 Private Relay Corrections

- Treat configured served Spaces as intent, not authority. Effective Spaces additionally require
  the provider Endpoint to hold the current `PRIVATE_RELAY_PROVIDER` capability.
- Track relay clients by `(EndpointId, ConnectionId)`. Endpoint-only tracking cannot distinguish
  concurrent sessions or correctly implement Iroh's per-connection disconnect callback.
- Recompute eligibility atomically, then disconnect clients only when their Endpoint loses every
  effective Space. Revocation in one Space must not tear down a session authorized by another.
- Persist provider-owned publication sequence separately and reserve it transactionally before
  signing advertisements. This prevents sequence reuse after restart and across concurrent calls.
- Keep relay configuration in Store-owned scalar/domain types. Convert to Iroh URLs, socket
  addresses, and runtime transport types only at the Net boundary.
- Private relay advertisements require HTTPS. Public relay fallback remains transport-only and
  never becomes Space authorization or advertisement data.
- TLS security must be verified through real client paths. Native TLS and external termination each
  need a successful HTTPS client connection, not just listener startup.
- Unix TLS key loading should open a bounded regular file with `O_NOFOLLOW`, then validate effective
  user ownership and exact mode `0600` on the opened handle before parsing.
- The repository rejects Rust files above 250 pure LOC. Split Store advertisement state from relay
  settings, and split advertisement validation coverage from publication/configuration coverage.
- Unix mode checks for secret files must retain the special permission bits (`mode & 0o7777`), not
  only the rwx bits, before requiring exact `0600`.
- Windows secret-file ACL checks must query the already-open handle. `GetKernelObjectSecurity` avoids
  path re-resolution; require current-user ownership, a protected DACL, and exactly one full-control
  allow ACE each for current user and SYSTEM, with no inherited or additional ACEs.
- Cross-crate validation cannot protect a public raw Store mutation. Make the validated Store token
  unforgeable, move all trust-boundary checks into its constructor, and expose persistence only for
  that token. A `compile_fail` doctest should lock the removed raw type's visibility.
