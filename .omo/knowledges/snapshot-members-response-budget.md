# Snapshot member projection and response budget

Analysis date: 2026-09-21. Reviewed commits d4c1781 and 632854d and current source. The initial analysis preceded the implementation recorded below.

## Confirmed behavior

- `ma2a-store/src/revision.rs` projects every signed member of every local Space. The actor maps these into `SnapshotSpaceView`; `api/snapshot/value.rs` serializes the complete nested member lists.
- `api/responses.rs::bounded_json` rejects successful responses above 65,536 bytes. IPC adds `runtime_boot_id` and checks the bound again. The IPC client independently enforces the same frame limit.
- Authenticated HTTP snapshot reads use IPC. An IPC failure becomes HTTP 503. SSE establishes its baseline and polls once per second through the same full snapshot path; oversized snapshots therefore affect event delivery as well as page loading.
- `space_show` currently builds the global runtime snapshot, selects one Space, and returns only the small `SpaceView`. It is not yet a detail query.
- `web/src/screens/spaces.tsx` consumes members and chain heads. `screens/derive.ts::peerList` also consumes signed members and feeds both Peers and Map. Removing members requires migrating all these consumers; missing details must not be represented as an authoritative empty member set.
- Store snapshot reads already reload and verify chains via `space_rows::load_chain`. Removing member projection alone does not remove chain replay work.
- The API schema allows 256 items per top-level collection; the frontend separately declares a 64-Space limit. Do not assume these declarations establish the same enforced runtime bound.

## Recommended repair

1. Separate global summaries from a bounded per-Space detail read. Keep members and their generation/hash in a detail object read in one SQLite transaction with its revision and local-membership check. Prefer a dedicated command/result to preserve existing small command results.
2. Keep the authenticated Web -> IPC -> actor -> Store path. Query one Space directly instead of constructing all Spaces and filtering afterward.
3. Cache detail by Space ID and chain hash when the summary budget permits retaining hashes. Return the head with members, reject stale asynchronous responses, and invalidate on membership changes/removal and runtime restart. If heads also move out of summaries, define a separate invalidation/version mechanism.
4. Fetch visible Space details on demand. Peers/Map require bounded-concurrency loading of relevant Space details and explicit partial/loading state. A missing observation never means a signed member is absent.
5. Add a small revision/boot stamp read for SSE; ordinary status currently lacks the boot ID needed for restart detection.
6. Audit the complete stamped response budget. Removing members fixes this amplification but cannot guarantee all schema-valid snapshots fit: connection histories, relay URLs, and nested covered Space IDs also grow. A complete capacity solution requires bounded pages/detail collections or a documented smaller enforceable aggregate budget, without silent truncation. Pagination needs a consistency token and a restart policy on state changes.
7. Update the Rust schema/hash, TypeScript models and validators, protocol documentation, and contract fixtures together. The current contract has exact compatibility; old clients require coordinated upgrades or an explicit version change.

## Verification for implementation

- Add regression coverage for 8 and more full Spaces through actual stamped IPC and authenticated HTTP, plus SSE baseline and revision progression.
- Cover maximum valid labels including quote/backslash escaping, complete 64-member details, concurrent membership updates, stale detail responses, Space removal, and unrelated revisions that should retain cached details.
- Exercise full aggregate response limits, including stamp/envelope overhead, connection history and relay coverage. Assert complete data or explicit pagination, never silent loss.
- Suggested commands: `cargo test -p ma2a-store`; `cargo test -p ma2a-runtime`; `cd web && bun run check`. These were not run for this analysis.


## Implemented repair and verification

- Removed member arrays from Store and API snapshot summaries, retaining head/hash/counts for cache invalidation. Added `space_details_fetch` / `space_details` and authenticated `GET /api/v1/spaces/{space_id}`. Details use a single-Space Store transaction and return `not_found` for unknown Spaces or nonmembers.
- Added `snapshot_stamp` through the actor and Store mailboxes. SSE now polls durable revision plus process boot ID without snapshot construction. Payload revisions also stamp the response envelopes for both new reads.
- The shared frontend controller loads details with a four-request concurrency ceiling, caches matching heads across unrelated revisions, invalidates on removal/head changes/disconnect/resync, and ignores stale or stopped requests. Spaces, Peers and Map display incomplete/loading/failure states instead of treating unavailable members as an empty authoritative set.
- Updated both exact schema copies, schema hash, Rust and TypeScript inventories, codecs, contract fixtures, and protocol documentation. This requires a coordinated Runtime/frontend upgrade; no protocol negotiation was introduced.
- Synthetic signed integration fixtures cover 16 full 64-member Spaces over actual IPC, authenticated HTTP, and SSE. Labels include 64 bytes of quotes/backslashes. Reconstructing the former inline-member payload confirms it exceeds 65,536 bytes; summary and detail responses fit. The test also covers authentication, missing Space, nonmember reads, and independent session mutations observed by SSE.
- A separate codec boundary test covers 64 full Space summaries with long escaped labels, maximum-width revision/generation numbers and the boot stamp, plus a complete 64-member detail and rejection of partial details.
- General aggregate pagination was not added. Connection history and relay coverage can still exceed the total response budget at sufficiently large combined sizes; no data is silently truncated. Store summary reads still verify/replay chains. These are separate remaining capacity/performance constraints.

Verified commands (2026-09-21):

```sh
cargo test -p ma2a-store -p ma2a-runtime
cd web
bunx tsc --noEmit
bunx vitest run src/runtime-details.test.ts src/runtime-controller.test.ts src/runtime-view.test.ts src/api/client.test.ts
bun test src/api/generated.test.ts
bun run build
```

Results: 149 Rust tests passed across 33 suites, 19 focused frontend tests passed, seven Bun schema tests passed, TypeScript checking passed, production build passed. Biome passed on all 14 changed frontend files. `git diff --check` passed.

Existing baseline checks requiring separate cleanup:

- Full frontend `bun run test` has six failures: five from `import.meta.dir` in schema tests under Vitest, and one credentials expectation (`include` versus actual `same-origin`). Both failing files reproduced the same six failures in an isolated checkout of unmodified HEAD. The schema suite passes under Bun.
- Ordinary Clippy is blocked by the existing `ma2a-net/src/relay_map.rs` `filter_map_bool_then` error. `--no-deps` exposes five existing `too_many_arguments` errors in snapshot constructors. `cargo clippy -p ma2a-runtime -p ma2a-store --all-targets --no-deps -- -A clippy::too_many_arguments` completes, with the existing `match_same_arms` warning in connection error projection. No lint suppression was added to production code.
- The first full-Space integration attempt used `space_create` merely to trigger an SSE update and encountered the existing two-second IPC deadline on that mutation. The final test uses session creation to advance durable revision, directly proving that stamp polling sees Store commits independent of actor-local status. It does not establish mutation performance under large Space sets.

## Aggregate transfer follow-up

The later Phase-1 consistency pass replaces aggregate rejection with a frozen
snapshot stream. See `phase-one-consistency-pass.md` and the current local API v1
contract: large snapshots and Space lists use ordered revision/boot-bound fragments,
with no truncation and no arbitrary cap on the number of Spaces. Per-frame size
remains 65,536 bytes; fragment worst-case encoded size is below 33,000 bytes.
The previous paragraph describing unresolved aggregate capacity is historical.
