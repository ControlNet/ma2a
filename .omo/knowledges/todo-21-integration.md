# Todo 21 Integration Knowledge

- A CLI E2E that parses stdout as JSON must explicitly request the command's JSON mode. `ma2a status` is human-readable by default; `ma2a status --json` returns the local API snapshot envelope with `/result/type = "snapshot"`.
- When rebasing a long accepted range, preserve semantic conflict choices and verify them with `git range-diff`, exact commit counts, merge audits, and repeated runtime gates before fast-forward promotion.
- Cargo verification can reorder dependency entries in `Cargo.lock` without changing resolution. Treat ordering-only lockfile changes as generated churn and restore them before finalizing the worktree.
