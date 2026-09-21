# Init password requirements and masked errors

Investigated on 2026-09-21 at d027c7c; updated below for the requested length-policy change.

- `crates/ma2a-store/src/web_auth.rs` now accepts 1 through 1,024 UTF-8 bytes for password derivation and verification, matching the API constructor. The previous Store minimum was 12 bytes. There is no character-class or strength validator in this path.
- `crates/ma2a-app/src/commands/ui.rs` requires the two prompt inputs to match byte for byte. Empty passwords and passwords over 1,024 bytes remain invalid.
- `init` dispatches the Set transition, which only succeeds if no verifier exists. Existing credentials require the Reset transition; reset invalidates existing sessions.
- `WebAuthService::change_password` maps Store errors, including invalid password length, to `AuthFailure::Internal`.
- `api/response_decode.rs::decode_ui_control_response` only understands success envelopes. An error envelope lacks `result` and is reported as invalid protocol input. The CLI then wraps this as `current-user control transport failed`, masking the real rejection. That message alone cannot identify password length, an already initialized state, or another backend failure.
- A safe diagnostic is `"$MA2A" --state-dir "$STATE" status --json`, inspecting `ui_auth.password_set`. Passwords should remain in the hidden prompts, never in diagnostic output or command arguments.

The implementation changes only the Store minimum and documents the byte bounds in the quickstart. Synthetic regression tests in `crates/ma2a-store/tests/password_bounds.rs` cover one byte, eleven bytes, 1,024 bytes, multibyte UTF-8, incorrect passwords, empty input, and oversized input. Before the change, the acceptance test failed with `InvalidPasswordLength` and the rejection test passed. Run `cargo test -p ma2a-store --test password_bounds` to verify both tests pass with the new policy. The masked error-reporting issue remains separate.

Verification: `cargo test -p ma2a-store --test password_bounds --test web_auth_atomicity` passed all five tests. `git diff --check` passed.
