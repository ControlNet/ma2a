# F1 Todo 15 evidence remediation

- The authoritative product binding is `c8676ea48a618c7ae1b377ad0cc4508fb8f9f5df`.
- Fresh focused proof is 14/14: RelayMap matrix 4/4, live address observation 5/5, Runtime reachability 3/3, and two focused actor tests 2/2.
- The evidence artifact uses JSON Lines to state eligibility versus home compatibility, exact `DegradedNoCommonHome`, Iroh-owned observed-home filtering, stable Endpoint identity, and `initial + 1` address publication.
- Cargo commands can reorder `axum`/`clap` and `tower`/`windows-sys 0.61.2` in `Cargo.lock`; restore the lockfile from HEAD after confirming that exact ordering-only diff.
- Rust LSP requests timed out, so fresh locked Nextest runs are the recorded executable fallback.
