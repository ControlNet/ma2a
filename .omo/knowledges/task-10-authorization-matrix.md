# Task 10 Authorization Matrix

- Endpoint authorization must be tested as a conjunction within one Space. Cross-Space allow/deny tests do not independently prove each membership, capability, policy, and revocation predicate.
- Echo policy exclusion needs a mutation-sensitive fixture where caller and target Echo capabilities are both present, no endpoint is revoked, exactly one shared Space exists, and only the policy grant is absent.
- Keep operation-wide cardinality/property coverage separate from predicate-specific regressions. The operation matrix is already in the 200-250 pure-LOC warning band, so focused policy tests belong in coherent sibling modules.
- A useful mutation proof changes only the predicate under test. Granting `PolicyGrants::ECHO` in the isolated denied fixture must turn authorization into success and make the denial assertion fail.
- Todo 10 final remediation commit: `f84e0ec55bc30984be186156a8d10e475082eff4`.
