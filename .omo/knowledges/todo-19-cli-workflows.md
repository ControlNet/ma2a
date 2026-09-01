# Todo 19 CLI Workflows

- Secret-bearing CLI values must cross only stdin, owner-controlled files, or hidden terminal prompts. Reject invite/password values in argv before Clap can reflect them.
- Invite creation is safest as a daemon operation that writes the canonical ticket with exclusive creation and mode `0600`; the response contains only non-secret Space metadata.
- Keep the local API schema, independent SHA-256, Rust fixtures, and generated TypeScript union synchronized in one change. The final inventory is 24 commands and 22 results with hash `11f17ae84296edbe1f326241fd46f6791b6ec03e79ad0691a95f528c739471d5`.
- Private relay configuration persists listener, public URL, served Spaces, and either native-TLS paths or explicit external termination. Key contents never cross CLI or IPC boundaries.
- `ui open` should discover a daemon-owned loopback URL through IPC and pass it directly to the platform opener without shell construction or embedded credentials.
- Process-level tests are the right acceptance surface for file modes, exclusive writes, non-TTY refusal, relay lifecycle, and browser opener behavior.
- Separate worktrees outside the session root may be rejected by LSP tooling; strict Clippy, compiler, tests, rustfmt, LOC checks, and direct process QA provide authoritative evidence.
- Invite redemption must bind validation to the opened file handle. Unix uses `NOFOLLOW`, regular-file identity, effective-UID equality, exact mode `0600`, and a bounded read; Windows opens the reparse point itself, rejects reparse attributes, and compares the handle security-descriptor owner SID with the current process user SID.
- Store mutation results that advance revision should return the committed revision as a typed value so Runtime state and emitted events cannot guess or reread it through a separate transaction.
- Persist user-facing Space labels in the signed local member entry and project that signed value into snapshots; replacing labels with Space IDs on restart loses durable state.
- Relay status must report actor-owned live server state and observed public relay state. Persisted private relay configuration must reconstruct the owned server during Runtime startup before status can claim it is online.
- Final remediation verification passed strict workspace Clippy, locked `xtask check` with 352 Rust nextest cases and 54 Web tests, focused process regressions, and direct terminal QA for help precedence, argv secret rejection, invite mode/prefix, durable labels, and relay restart restoration.

## 2026-08-31 Independent acceptance re-audit at c947557

VERDICT: REJECT

- Unix invite redemption opens a FIFO with blocking `O_RDONLY` before checking `metadata.is_file()`. A hands-on `timeout 3 ma2a ... redeem --file <fifo>` exited 124, so a non-regular input can hang instead of failing before ticket parsing; committed security tests cover mode `0644` and symlink only.
- Windows invite validation rejects reparse points and compares owner SID, but requests only `OWNER_SECURITY_INFORMATION` and never validates the DACL. A current-user-owned file readable by other principals therefore passes despite the owner-only contract; native Windows execution was unavailable, and the AArch64 MSVC check stopped in `blake3`, `ring`, and `libsqlite3-sys` before MA2A source compilation because Windows C/MSVC tools and headers are absent.
- The corrected snapshot is still not fully authoritative: `Actor::snapshot` hard-codes `EchoSummaryView::new(0, 0)` despite actor-owned Echo audit/metrics, and reports `reachability.direct = true` from the immutable `Connectivity::DIRECT_ONLY` capability rather than an observed direct path. No behavior-sensitive test exercises either field.
- Invite Store revision propagation, actor revision update, Runtime event emission, SSE polling invalidation, durable signed Space labels, Private Relay restart restoration/live status, and Public Relay observed status are source-connected. Focused CLI tests passed 12/12; relay snapshot test passed; rustfmt, strict workspace Clippy, LOC, diff checks, Secret Guard tracked/gitignore, and locked aggregate `xtask check` passed with 352/352 Rust and 54/54 Web tests.
- LSP diagnostics were attempted for changed App and Runtime files and rejected the isolated `/tmp` worktree as outside request cwd; compiler, Clippy, rustfmt, nextest, and aggregate checks were used as fallback. Wrong-owner Unix behavior was reproduced with `/etc/hosts` (UID 0, mode 0644) returning exit 2; an owner-only regular malformed ticket reached parsing and returned non-success; mode `0644`, symlink, and directory inputs failed before parsing.
- Help precedence introduces a secret-reflection regression: `ma2a space invite redeem argv-probe-marker --help` exited 2, but stderr included `unexpected argument 'argv-probe-marker'`. The global information-flag bypass in `reject_secret_argv` lets Clap inspect and echo an unsupported ticket argv instead of returning the generic non-reflecting secret-channel rejection; the committed help test covers only bare `--help`.

## 2026-08-31 Rejection corrections at 9d6a72a

- Unix invite redemption now rejects non-regular paths through `symlink_metadata` before the blocking open. The process regression creates a mode-0600 FIFO and proves the CLI exits with code 2 within two seconds.
- Windows invite redemption now requires current-user ownership, a protected DACL, and exactly two explicit full-control allow ACEs for the current user and Local System. Pure policy tests cover the accepted shape, inherited rules, and an extra principal; the Windows-only FFI modules compile for `aarch64-pc-windows-msvc` in an isolated `windows-sys` fixture.
- Runtime snapshots project direct reachability from actor-consumed Iroh observations, initialized false before observation, without advancing durable revision for a direct-only telemetry change. The relay scenario proves false before observation and true after observation.
- Runtime snapshots derive Echo success/failure totals from the actor-owned bounded Echo audit log. A live Runtime regression records an unavailable Echo outcome and observes one snapshot failure.
- Positional invite values are rejected before Clap processes `--help`, while bare redeem help remains available. Direct binary QA showed the generic rejection without reflecting the sentinel.
- Correction commits preserve rejected HEAD `c947557`: `fd640b9`, `b5f404b`, `cc443f8`, `ea55f3b`, `5c2b9d7`, and `9d6a72a`.
- Final verification passed `cargo test -p ma2a-runtime`, `cargo test -p ma2a-app`, `cargo fmt --all -- --check`, host `cargo check --all-targets`, strict workspace Clippy, locked `xtask check`, 54 Web tests, tracked Secret Guard, and gitignore coverage. LSP diagnostics remained unavailable because the isolated worktree is outside the request cwd; native Windows execution and Miri coverage of Windows FFI were unavailable.

## 2026-08-31 Open-boundary and Echo revision authority corrections

- A deterministic Unix regression replaces a checked owner-only regular invite file with a mode-0600 FIFO immediately before the real open. It failed before the fix with a 500 ms timeout and passes with `O_NONBLOCK`; validation and bounded reading remain bound to the same opened handle.
- The built task-worktree CLI passed direct terminal QA: `--help` exited 0; an owner-only regular malformed ticket reached normal redemption processing; a FIFO exited 2 inside a two-second guard with `invite file must be an owner-only regular file` instead of hanging.
- Echo workers now return payload-free audit outcomes to the single-owner actor. The actor durably advances the Store revision, records the bounded audit entry, adopts the revision, emits the Echo summary event, and only then releases inbound or outbound response visibility.
- Runtime regressions failed before the fix because snapshots at one revision changed failures from 0 to 1. They now prove an Echo outcome advances the authoritative snapshot revision and equal-revision snapshots have equal Echo summaries.
- `cargo test -p ma2a-net -p ma2a-runtime -p ma2a-app`, strict affected-crate Clippy, all-target checks, rustfmt, and `cargo xtask check` passed. The aggregate gate included LOC policy, dependency policy, Rust verification, and 54/54 Web tests.
- LSP diagnostics were attempted for every changed Rust file and remained unavailable because the isolated `/tmp` worktree is outside the request cwd; compiler, Clippy, tests, rustfmt, aggregate checks, and manual CLI QA supplied verification evidence.

## 2026-08-31 Final argv secrecy and Echo response revision corrections

- Pre-Clap secret scanning now rejects inline `--password=<value>` and positional invite values anywhere after `space invite redeem`, while skipping only the legitimate value immediately following `--file`. Bare redeem help remains available.
- Process regressions cover positional invite values after `--stdin`, positional invite values after `--file <path>`, and inline password values. All 8 `cli_secrets` tests pass.
- Direct built-binary QA exercised all three unsupported forms. Each exited 2, emitted only `error: secret values are not accepted in argv; use secure input`, and did not reflect its unique probe; `space invite redeem --help` exited successfully and rendered usage.
- IPC response revision selection now treats `echo_call` like other actor-mutating operations, so the command envelope observes the actor revision after the Echo outcome has been durably recorded and published. The focused classification regression and all 6 Echo-focused Runtime tests pass.
- `cargo fmt --all -- --check`, affected-crate and strict workspace Clippy, LOC policy, and locked `cargo run --locked -p xtask -- check` passed. The aggregate gate included dependency policy, Rust verification, and 54/54 Web tests.
- LSP diagnostics were attempted for all three changed Rust files and remained unavailable because the isolated `/tmp` worktree is outside the request cwd; compiler, Clippy, focused tests, aggregate checks, and direct process QA supplied verification evidence.

## 2026-08-31 Terminal independent-review correction

- Five independent lanes completed against `7f50a2d`: goal and hands-on QA passed; context and security review found two concrete CLI-boundary gaps; code quality passed while noting the same untested edge surface.
- The valid Clap spelling `space invite redeem --file=PATH` was incorrectly rejected by the pre-Clap scanner. It now counts as a supported secure file input and reaches normal owner-only file validation.
- Unknown dash-prefixed values in the `space invite redeem` and `ui password set|reset` tails could reach Clap and be reflected in diagnostics. Those closed command tails now allow only their documented secure inputs, global state-dir syntax, and information flags; every other token receives the generic non-reflecting secret-channel error.
- Process regressions first failed for both reflected dash-prefixed markers and inline file syntax, then passed after the correction. The focused suite now passes 11/11 tests.
- Direct built-binary QA confirmed both dash-prefixed probes exit 2 without reflection, `--file=missing.ticket` reaches file validation with `invite file must be an owner-only regular file`, and bare redeem help remains available.
- Rustfmt, strict App Clippy, LOC policy, revision-focused Runtime tests 6/6, Echo-focused Runtime tests 6/6, and locked aggregate `xtask check` passed, including dependency policy and 54/54 Web tests. LSP remained unavailable for the isolated `/tmp` worktree because of the known request-cwd limitation.
