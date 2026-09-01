# Todo 4 Independent Re-verification

## 2026-08-29 - Verdict: REJECT

### Blocking findings

- The TypeScript/schema equivalence gate checks property names only. `ExactKeys` in
  `web/src/api/generated-models.ts` cannot prove field types or requiredness, and
  `generated.test.ts` compares only each schema type's `required` array with `MODEL_FIELDS`.
- In a disposable copy, changing `EndpointView.runtime_version` from `string` to `number` passed
  both `bunx tsc --noEmit` and `bun test`.
- In a second mutation, making required `EndpointView.online` optional, changing
  `LocalApiSuccessResponse.result` from `CommandResult` to `string`, and changing the
  `spaces_changed.changed.space_ids` payload to optional `EndpointId[]` all passed both Web gates.
- The Rust response/event schema tests also compare object keys only. Mutating encoded success and
  event `revision` values from JSON integers to JSON strings passed
  `cargo test --locked -p ma2a-runtime --test api_schema_shapes` (2/2 tests).
- Therefore the complete schema is descriptive but not mechanically equivalent to the Rust and
  TypeScript wire types. Meaningful primitive, optionality, nested payload, response-envelope, and
  event drift can ship with every claimed cross-language gate green.

### Candidate verification

- `GIT_MASTER=1 git diff --check a8f1215`: passed.
- `cargo test --locked -p ma2a-runtime --test api_contract --test api_canonical_input --test api_schema_shapes`:
  passed 13/13 tests.
- `cd web && bun run check`: Biome, strict TypeScript, and 3/3 tests passed.
- `cargo fmt --all -- --check`: passed.
- `cargo run --locked -p xtask -- check`: completed successfully, including Rust, Web, dependency,
  advisory, license, source, and unused-dependency gates; dependency duplicate warnings remain
  informational.
- `lsp_diagnostics` was attempted for every changed Rust and TypeScript source/test file but was
  rejected before startup because `/tmp/opencode/ma2a-todo-4` is outside the session cwd.

### Required remediation

- Generate Rust and TypeScript contract types from one schema, or add exhaustive machine checks that
  compare every field's type/ref, requiredness/nullability, literal union, array item type/bound, and
  every command/result/response/event discriminated payload against that schema.
- Add mutation tests or equivalent negative fixtures proving representative type, optionality,
  envelope, nested result, and event payload drift fails the release gate.

Todo 4 remains blocking despite the candidate's normal test suite passing.

## 2026-08-29T09:05:20+10:00 - Remediated candidate re-verification

### Clean control

- `bun test src/api/generated.test.ts`: passed 3/3, including the recursive TypeScript schema gate.
- `cargo test --locked -p ma2a-runtime --test api_contract --test api_canonical_input --test api_schema_shapes`: passed 15/15.
- `GIT_MASTER=1 git diff --check a8f1215`: passed before and after mutation work.

### Independent mutation matrix

All TypeScript mutations used `bun test src/api/generated.test.ts` in a disposable copy.

| Mutation | Expected schema seam | Observed result |
|---|---|---|
| `EndpointView.runtime_version: string` to `number` | `type.endpoint_info.payload.runtime_version` primitive kind | Failed as intended: `expected string` |
| required `EndpointView.online` to optional | requiredness | Failed as intended: `requiredness mismatch` |
| `LocalApiSuccessResponse.result: CommandResult` to `string` | success-envelope discriminated result payload | Failed as intended: `type: variant count mismatch` |
| `LocalApiSuccessResponse.request_id: RequestId | null` to `RequestId` | nullability | Failed as intended: `expected nullable type` |
| required `spaces_changed.changed.space_ids: readonly SpaceId[]` to required `readonly EndpointId[]` | semantic `space_id` reference/brand | **Remained green: 3/3 passed** |
| required `spaces_changed.changed.space_ids: readonly SpaceId[]` to scalar `SpaceId` | array identity | **Remained green: 3/3 passed** |
| `EndpointView.endpoint_id: EndpointId` to `SpaceId` | semantic `endpoint_id` reference/brand | **Remained green: 3/3 passed** |
| exported `LocalApiResponse` union to `string` | client-visible success/error envelope union | **Remained green: 3/3 passed** |

Rust mutation command:
`CARGO_TARGET_DIR=/tmp/opencode/ma2a-todo-4/target cargo test --locked -p ma2a-runtime --test api_schema_shapes every_result_and_success_envelope_recursively_matches_the_machine_schema -- --exact`.

| Mutation | Expected schema seam | Observed result |
|---|---|---|
| success response `revision` JSON integer to JSON string | `responses.success.revision` recursive numeric-kind validation | Failed as intended: `expected unsigned integer` |

After restoring the disposable copy, the TypeScript gate passed 3/3 and Rust schema-shape tests passed 4/4.

### Verdict and smallest remediation

- **Verdict: REJECT.** Primitive kinds, requiredness, nullability, discriminated payloads, concrete envelopes, and Rust serialized numeric kinds are now mechanically checked, but semantic references/brands and array identity are not. The exported `LocalApiResponse` union is client-visible envelope contract and is also outside the checker.
- Preserve schema reference identity before recursively resolving primitive nodes: map `endpoint_id`, `space_id`, and `request_id` to their exact exported branded TypeScript types and reject cross-brand substitution.
- In the array branch, require an actual `Array`/`ReadonlyArray` type before checking its item type; a numeric index type alone accepts branded scalar strings.
- Explicitly assert exported `LocalApiResponse` is exactly the union of `LocalApiSuccessResponse` and `LocalApiErrorResponse`.
- `lsp_diagnostics` was attempted for all remediated checker/model/generated and Rust validator/fixture/shape-test files, but every request was rejected because the `/tmp/opencode/ma2a-todo-4` paths are outside the session cwd.

Todo 4 remains blocking for Todos 7, 18, and 19.

## 2026-08-29 final remediation re-verification

### Baseline and clean controls

- Recorded the existing dirty candidate state before verification and again after cleanup. No candidate product file was edited by this verification.
- `GIT_MASTER=1 git diff --check a8f1215` passed before and after all probes.
- A disposable copy was byte-identical to the candidate for `generated.ts` and `generated-models.ts`; its clean baseline passed 8/8 Web tests.
- Candidate focused Rust controls passed 15/15: `api_canonical_input` 3/3, `api_contract` 8/8, and `api_schema_shapes` 4/4.

### Corrected targeted mutation matrix

Each mutation was independently applied to the disposable copy. Every probe first passed `bunx tsc --noEmit`, then ran only `generated TypeScript wire types recursively match the machine schema`, and restored touched sources byte-for-byte before the next probe.

| Mutation | `tsc` | Checker | Intended signal |
| --- | ---: | ---: | --- |
| `space_ids: readonly SpaceId[]` -> `readonly EndpointId[]` | 0 | 1 | `type.spaces_changed.changed.space_ids[]: reference identity mismatch` |
| `space_ids: readonly SpaceId[]` -> scalar `SpaceId` | 0 | 1 | `type.spaces_changed.changed.space_ids: expected array` |
| `EndpointView.endpoint_id: EndpointId` -> `SpaceId` | 0 | 1 | `type.endpoint_info.payload.endpoint_id: reference identity mismatch` |
| `LocalApiResponse` exact union -> `string` | 0 | 1 | `LocalApiResponse: union identity mismatch` |
| nullable success `RequestId` reference -> `EndpointId` | 0 | 1 | `responses.success.request_id: reference identity mismatch` |
| `space_ids` array -> non-array numeric-index `IndexedSpaceIds` | 0 | 1 | `type.spaces_changed.changed.space_ids: expected array` |
| `LocalApiResponse` -> `LocalApiSuccessResponse | EndpointId` | 0 | 1 | `LocalApiResponse: union identity mismatch` |
| exact response union with members reordered | 0 | 0 | targeted checker passed, proving order-insensitive membership |
| response union missing `LocalApiErrorResponse` | 0 | 1 | `LocalApiResponse: union identity mismatch` |
| duplicate-normalized success member union | 0 | 1 | `LocalApiResponse: union identity mismatch` |

- All ten probe outcomes matched expectations and every restoration comparison passed.
- An initial mutation setup used a stale search string and changed no source; its unchanged green result was discarded before the corrected targeted matrix above.

### Quality gates

- `bun run check` passed: Biome clean, TypeScript clean, and 8/8 tests passed.
- `bun run build` passed with a production Vite build.
- `cargo fmt --all -- --check` passed.
- Focused Rust tests passed 15/15.
- `cargo run --locked -p xtask -- check-loc` passed; `web/src/api/schema-type-check.ts` has 242 pure LOC, below the 250 maximum.
- `cargo run --locked -p xtask -- check` passed end to end, including pins, LOC/support policy, rustfmt, clippy with warnings denied, workspace nextest, cargo-deny, cargo-machete, frozen Web install, Biome, TypeScript, and 8/8 Web tests. Cargo-deny emitted duplicate-version warnings for Windows target crates but completed with advisories, bans, licenses, and sources all OK.
- LSP diagnostics were attempted for `schema-type-check.ts`, `schema-type-model.ts`, `generated.test.ts`, and `api_schema_shapes.rs`; each request was rejected because `/tmp/opencode/ma2a-todo-4` is outside the session request cwd. The successful compiler, clippy, Biome, TypeScript, test, and aggregate gates cover these files.
- The disposable copy, mutation runner, and command/status journal were removed; no `ma2a-todo4-final-reverify*` or mutation-runner artifacts remain under `/tmp/opencode`.

### Final verdict

- **VERDICT: APPROVE.** The remediation now mechanically rejects semantic brand substitutions, proves actual array identity rather than numeric-index compatibility, and enforces exact order-insensitive `LocalApiResponse` membership while rejecting missing, duplicate-normalized, and unrelated members. Clean controls, durable tests, focused mutation probes, direct quality gates, and the aggregate workspace gate all pass.
- Todo 4 is no longer blocking Todos 7, 18, and 19.

## 2026-08-29 - Local commit record

The independently approved remediation was committed from `/tmp/opencode/ma2a-todo-4` without changing product bytes. Dependency order and grouping:

1. `908a64d02a54609da05a9e980d464c517fcf3299` `fix(runtime): reject noncanonical local api requests` - serde manifests/lock, strict root-object decoding, codec integration, lowercase identifier enforcement, and direct canonical-input regressions.
2. `f987501f9016afd54404d59e0f42a1961b614a23` `test(runtime): add schema validation primitives` - foundational recursive schema node checks.
3. `504d35e8bb59cb138aedc8ed65c45f411facf76c` `test(runtime): add recursive schema validator` - recursive command/response/event validation built on the primitives.
4. `ed80e7d9f0b476736fc7df0bc4ac97c6e2c3b3e1` `test(runtime): add complete api contract fixtures` - exhaustive command, result, snapshot, and event fixtures.
5. `b893202c6a38d2e1f04124434398309e854412ff` `feat(runtime): freeze exact local api wire schema` - stable error names, response encoding, canonical schema, hash checks, and direct recursive Rust proof.
6. `a513e28bec6f812e462541adeebe7ae454997e45` `test(web): model local api schema types` - TypeScript compiler-model foundation.
7. `6ccc47175088563ecd663b2136f081c018de732e` `feat(web): enforce exact generated api types` - generated models/schema, recursive checker, exact response union, brand/array checks, and direct mutation regressions.
8. `9f616c74f7fbfcd8e689241f91e53dcf2238e589` `docs(protocol): specify exact local api contract` - externally reviewable exact-version protocol rules.

Verification record: every staged group passed `git diff --staged --check`, complete staged diff inspection, and the staged secret scan. `git diff a8f1215..HEAD --check` passed; the final range contains exactly 22 intended product files; tracked and staged worktree diffs are empty. The pre-commit and post-commit aggregate SHA-256 over those 22 product paths both equal `0121194c7d2a4e31d649754381fa19cc85dbdf892cc11584295eb2f779ff07b3`. The only remaining worktree path is intentionally excluded and untracked: `.omo/knowledges/todo-4-schema-remediation.md`.
