# Todo 12 Adversarial Review

- A validated record cache needs its own sequence monotonicity guard even when persistence already
  rejects rollback: validated values can arrive out of order after concurrent or delayed work.
- Lookup-time validity must enforce both sides of the interval, `issued_at <= now < expires_at`,
  because validation-time clock state does not remain authoritative after clock rollback.
- Replacing keyed authorization state from a vector must reject duplicate keys before taking the
  write lock; silently collecting duplicates makes security state depend on input order.
- Todo 12 final remediation is recorded by commits `dad558c` and `fa9a38f`; the locked aggregate
  gate passed 138 Rust tests and 41 web tests.
