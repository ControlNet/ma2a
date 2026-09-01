# OpenCode `cyber_policy` SSE Failure

Date: 2026-08-30

## Scope

- OpenCode session: `ses_fb7cf84e2ffeXgO475zKkcH16U`
- OpenCode version: `1.17.18`
- Provider/model: custom OpenAI-compatible `codex/gpt-5.6-sol`

## Symptom

The final assistant turns completed with `finish=unknown` and zero input, output,
reasoning, and cache tokens. The UI showed no model reply and no useful error.

## Confirmed cause

The streaming endpoint returned HTTP 200, emitted normal `response.created` and
`response.in_progress` events, and then emitted:

```text
event: error
code: cyber_policy
```

OpenCode did not surface this SSE error in the stored assistant message; it only
recorded a `step-finish` with reason `unknown`.

The offending context was isolated to the `poc-engineer-a` member prompt created
by the `todo-13-rebased-security-review` team tool call. The prompt combined
phrases such as "PoC engineer against", "reproduce vulnerabilities", "exploit
attempt", identity-spoofing scenarios, arbitrary persistence, and requests for
exact commands. The other four member prompts were individually accepted.

## Safe mitigation

Keep the authorized defensive goals, but express them as negative security
invariants and regression tests. For example:

- Verify that unsafe TLS key modes are rejected.
- Verify that unvalidated relay advertisements cannot be persisted.
- Verify that signer identity must match the claimed provider.
- Verify sequence monotonicity, expiry, capability, and revocation checks.
- State local-only scope, no public-network access, and no product or Git-history
  mutation.

This defensive rephrasing was accepted by the same endpoint and model.

Because recent tool-call arguments remain in the session context, sending a
benign follow-up such as `continue` can trigger the same policy error. Prefer a
new session with a clean handoff summary that omits the flagged wording instead
of repeatedly retrying the contaminated context.
