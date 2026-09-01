# Problems — ma2a-phase-0-1

Unresolved blockers and technical debt discovered during work on this plan.

_Auto-scaffolded by /start-work. Append new entries below - never overwrite._

---

## 2026-08-29T06:58:37+10:00 - Independent Todo 4 adversarial verification

- Blocking: the checked-in local API schema/hash inventories names and a few top-level field names but does not define or compare complete command fields, response/error envelopes, result payloads, nested snapshot models, or event payload structures. Handwritten TypeScript models can drift while all current golden/hash tests pass.
- Blocking: generated TypeScript has no successful-response or error-response envelope types, and neither Rust nor TypeScript performs response/event cross-language roundtrips.
- Blocking: Rust error serialization is not mechanically exhaustive because unknown future `ProtocolError` values fall through to `internal`.
- Contract mismatch: uppercase hexadecimal IDs are accepted and normalized despite the frozen schema/documentation requiring lowercase; duplicate JSON version members are accepted using the last value.
- Exact-version rejection, canonical retry idempotency/conflict ordering, documented size limits, event gap handling, strict compiler/toolchain gates, and secret scans passed independent live probes, but they do not compensate for incomplete schema equivalence.
- Verdict: `needs-fix`; Todo 4 must continue to block Todos 7, 18, and 19.

## 2026-08-29 - Independent Todo 4 findings resolved

- Replaced the name-only golden with a complete 9,987-byte machine schema covering every command field, nested model, result payload, success/error envelope, event payload, literal, nullability rule, numeric width, text bound, and collection bound; new SHA-256 is `05989fefdb0ddc90db12b89c2f20c63d21b47c19edb5dd04e84d9775d9bc31ac`.
- Added TypeScript success/error response envelopes and exact `keyof` field inventories cross-checked against schema-required fields, plus Rust response and all-nine-event serializer/schema shape tests.
- Moved protocol error names onto `ProtocolError::name()` with an exhaustive private-enum match; removed the equality-chain fallback.
- Added a duplicate-aware root JSON visitor and lowercase-only ID decoding. Duplicate `version` is `invalid_input`; a single incompatible version still wins before duplicate non-version fields and all callbacks remain zero.
- Full Rust/Web gates, LOC, dependency audits, manual success/error driver, and focused secret scans pass. LSP remains unavailable because the isolated `/tmp` worktree is outside the session cwd.

## 2026-08-29 - Todo 4 recursive drift-gate remediation

- A second independent verification correctly rejected the field-name-only proof: `runtime_version: number`, optional `online`, `result: string`, optional/wrong-type `space_ids`, and string-valued event `revision` all passed the former gates.
- Replaced the Web `MODEL_FIELDS`/`ExactKeys` seam with a compiler-API walker that recursively compares exported commands, all results, both response envelopes, nested models, and all events against `LOCAL_API_SCHEMA_JSON`.
- Replaced Rust key-only shape assertions with recursive validation of all 21 encoded commands, all 21 serialized results, both envelopes, all nine errors, and all nine events, including references, literals, nullability, requiredness, exact fields, arrays, bounds, enums, numeric kinds, identifier formats, and HTTPS credential exclusion.
- Re-ran each verifier counterexample as a disposable mutation. All five now fail at the expected nested schema path; the clean focused Rust and Web gates pass after restoration.

## 2026-08-29 - Todo 4 semantic TypeScript gaps resolved

- Reproduced four remaining verifier mutations before editing: cross-brand `space_ids`, scalar `space_ids`, cross-brand `EndpointView.endpoint_id`, and unrelated `LocalApiResponse` all passed the former 3-test gate.
- The checker now preserves branded schema-reference identity, proves actual `Array`/`ReadonlyArray` identity before element validation, and binds `LocalApiResponse` to the exact success/error alias union.
- Durable temporary-source mutation tests reject all four counterexamples by `SchemaTypeMismatch` class without relying on natural-language error assertions. Direct post-fix mutations each exited non-zero at the expected structural checker branch.

## 2026-08-30 - Todo 14 remaining delivery operation

- The branch still requires the planned destructive history rewrite to remove the committed `.omo/knowledges/todo-14-control-sync.md` from the full Todo 14 range, followed by atomic commits. Product behavior and verification are complete; history mutation was not performed implicitly.

## 2026-08-30 - Todo 14 history finding resolved

- `GIT_MASTER=1 git log --all --oneline -- .omo` returns no commits, so no destructive history rewrite is required. The correction slice can be delivered with new atomic commits only.
