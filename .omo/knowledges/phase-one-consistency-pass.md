# Phase One consistency pass (baseline 2b6ee097)

This is a chronological implementation and validation record. Earlier work-in-progress
notes are retained as evidence; the later audit and final verification sections
record their resolution. The six requested workstreams keep SQLite authoritative,
retain explicit projection completion, and preserve the accepted relay publication
and genesis-owner signing models.

## Source and completion contract audit

SQLite owns desired durable state. Actor state and owned resources are applied
projections. A RequestId reservation must precede every potentially effectful
operation, and cannot be removed merely because execution returned an error.
A lost response, completion failure or crash is not proof of no effect.

| Mutation | First possible effect / durable commit | Subsequent fallible work at baseline |
| --- | --- | --- |
| Space create | authority key file; chain transaction | lookup, candidates, access, signed publication, response |
| Space invite | invitation transaction | secret ticket file creation/write; snapshot/response |
| Space redeem | remote owner redemption transaction | bootstrap transport/validation, candidate transaction, lookup/candidates/publication |
| Space revoke | owner chain transaction | membership query, lookup/access/candidates/publication, stale snapshot response |
| Space leave | remote owner revocation transaction | response transport, local chain transaction, projections |
| Private/public relay configuration/disable | new server bind; configuration transaction | old server shutdown, advertisements, candidate map, response |
| Explicit control sync | network exchange and received artifact transaction(s) | projections, completion/status response |
| Echo | remote operation; local audit transaction | audit completion, UTF-8/API encoding |
| UI init/password set/reset | password/session transaction | control result, Actor revision adoption, response |
| Session revoke all | session transaction | Actor revision adoption, response |
| Graceful shutdown | durable replay completion, then lifecycle signal | transport delivery and shutdown signal |

Baseline dispatcher aborts reservations on every execute error for all of these.
Baseline replay retention also deletes the identity/fingerprint when evicting a
completed response. Both reopen execution despite an earlier effect. Pending
reservations already survive process restart and prevent re-execution.

Deterministic initial regression: an existing directory as the invitation output
path fails create_new after the invitation transaction. Two identical calls create
two invitation rows on 2b6ee097 (`ma2a-phase1-replay-before.log`).

## Work tracking

All six workstreams and final audit remain in scope. Initial baseline enrollment
stress executes the three reported unstable E2E scenarios 20 times each, with normal
CI nextest concurrency and no retry setting; logs are in
`/tmp/ma2a-phase1-enrollment-before`.

## Replay contract implemented

Mutation failures carry typed effect knowledge: NotStarted, Committed(revision),
or Indeterminate. Ordinary error conversion is conservatively Indeterminate.
Only a proven NotStarted error releases admission. A committed or indeterminate
error is retained as an exact terminal response; errors warn that effects may have
occurred and the same request will not execute again. If terminal persistence or
response encoding fails, admission remains Pending. Validation before admission
has no effects. Owner-removal rejection explicitly proves NotStarted.

The invitation Actor now returns the Store's CreatedEnrollmentInvite internally,
so file-publication failure is marked with the exact committed revision. The
public ticket-returning Runtime convenience method is preserved.

Schema 6 introduces local_mutation_fences: permanent RequestId/fingerprint pairs,
seeded from every surviving schema-5 replay row and transactionally inserted on
admission. The bounded response cache can evict a response without losing its
fence. Such requests remain unavailable (or conflict on changed fingerprints),
never executable. Fences grow with distinct admitted mutations; there is no safe
age-based deletion policy under the required lifetime idempotency contract. IDs
already evicted before migration cannot be recovered retroactively.

Crash matrix: before admission -> no execution; after admission and before/after
any effect -> Pending prevents execution; after terminal persistence -> replay;
after response delivery loss -> replay. Retained graceful-shutdown responses do
not send a new shutdown signal to a later process.

The three enrollment baseline scenarios ran 20 times each, 0 failures / 60
executions. This does not disprove the previously observed intermittent failures;
stage diagnostics and deterministic completion regressions are still required.

## Authoritative response receipts

The deterministic revoke interleaving pauses a request before its mutation, adds
another member through the Actor, then resumes removal. Baseline returned one
member while the committed chain contained two. Responses now use the chain and
revision from the removal receipt, without a pre-read counter adjustment.

Created Spaces likewise return CreatedSpace. Invitation receipts carry the Space
chain read inside the invitation transaction, never a later snapshot. Credential
changes and session revocation return Committed<T> with the transaction revision;
no-op session revocation returns the revision and password presence from its read
transaction without advancing revision. Echo completion carries its audit revision.
Targeted control synchronization already requires that exact peer in the completed
round, so its response uses that completion instead of a second status query.

CommandResult carries internal revision metadata (not a wire-schema field change).
The dispatcher adopts and returns that revision while preserving the encoded
receipt payload. Enrollment and relay configuration adapters still need conversion
alongside their completion/resource reconciliation changes; the legacy fallback is
not the final contract and must be removed before this pass is complete.

## Enrollment completion work in progress

A deterministic Runtime test injects Interrupted after the candidate bootstrap
transaction and before lookup refresh. Before the fix, SQLite membership existed
but the error only said `enrollment exchange failed`. The revised error carries
its local committed revision and a non-secret stage, allowing IPC terminal replay
to retain a truthful committed-error response.

Candidate completion retains a set of owner targets and the current completion
stage in maintenance state. A subsequent join cannot overwrite an earlier target.
The normal periodic tick retries lookup, relay candidates/access and idempotent
signed publications. Targeted control is scheduled once after convergence, then
the pending set clears. No invite is resubmitted by this retry. Process restart
rebuilds projections from Store; pending targeted work is process-local.

Enrollment persistence derives membership and the accepted chain inside the same
control-batch transaction, before commit. Its successful receipt carries that chain
and revision directly to the local API, without a later snapshot. Existing chain
rollback/fork validation is unchanged. Bootstrap retry at the same state is a
no-op transaction, without a revision increment.

Owner redemption completion now retains membership-dependent work before lookup,
access/candidate/publication refresh. Recognized temporary errors remain pending;
invariant/integrity errors propagate from the Actor. Exact owner invite redemption
replay remains controlled by the existing owner transaction. Transport diagnostics
use fixed stage labels only; Iroh's connect call combines dial and ALPN negotiation.

The same membership follow-up marker is used for local Space creation/removal and
departure, keeping projection completion pending after a committed mutation fails.
RuntimeError can carry a committed revision; public error classification is not
broadened. General control-round completion paths still need the final audit.

Intermediate enrollment stress (before the reliability diagnosis is finished):
3 failures / 60 executions. All three are the existing replay test's owner
revision assertion, 17 versus 16, after the expected candidate/RequestId denials.
Cold bootstrap and private-Space injection passed all 20 executions each. Logs:
`/tmp/ma2a-phase1-enrollment-after`. This is NOT the accepted final after-result.
A test-only SQLite revision trigger is being used to identify the extra write;
keep the strict revision assertion while diagnosing it. Initial trace shows
unchanged generation 1, two address records at sequence 0, and revision 17.
The test-only trigger contains public counts/Endpoint identifiers, never tickets.

The two deterministic completion regressions passed 20 runs each (40 executions)
in `/tmp/ma2a-phase1-completion-repeat`. Focused app enrollment suite passed 5/5;
Store bootstrap receipt tests passed 3/3. Strict Runtime/Store/Net Clippy passed.

Replay stress diagnosis was confirmed, not inferred: a SQLite test-only trace
recorded revision 16 with one address record at sequence 0, then revision 17 with
two records at sequence 0. Manifest generation and observation counts were
unchanged. The newly learned record belonged to the successful candidate; the
initial targeted sync had not finished before the owner was immediately restarted.
The replay test now completes the real common-Space control exchange on both peers
and asserts the candidate address exists before taking the denial baseline. Its
exact revision-equality and generation-equality assertions are retained. Temporary
SQL diagnostic instrumentation was removed; logs remain in
`/tmp/ma2a-phase1-enrollment-diagnosis2/run-8.log`.

## Final audit follow-ups identified (not yet fixed)

- `finish_control_round` and inbound `finish_control_call` still drop typed errors
  into `.is_ok()` / `.is_err()`, and can lose ControlChanges scheduling after a
  committed batch. `adopt_control_memberships` reads memberships separately from
  the outcome revision and silently ignores read failure. These need a focused
  deterministic completion regression and retained follow-up state during the
  final audit; do not report the entire pass complete before addressing them.
- `StoreBackend::revoke_owned_space_member`, `advance_owned_space`, and
  `persist_departure` still read memberships after chain commit. Move that
  projection into the relevant transaction or otherwise return an explicit
  committed failure with retained recovery work.
- Private Relay configuration still uses spawn-before-stop; its fixed-listener
  regression remains intentionally failing until workstream 4 is implemented.

After the real control-convergence precondition, the same three stress scenarios
passed 60/60 executions (20 rounds), preserving all denial/revision/generation
assertions. Logs: `/tmp/ma2a-phase1-enrollment-after-barrier`. The intermediate
`enrollment-converged` directory contains compilation failures from an attempted
use of a crate-private helper, not test executions; it is excluded from statistics.
Final code uses the existing public sync_control method and explicitly verifies
the candidate's address is durably present. App all-target/all-feature strict
Clippy passed. These are workstream-level checks, not final-head CI acceptance.

## Private Relay desired/applied resources

The fixed-listener regression failed before workstream 4: replacement tried to
bind before releasing its own server. Store relay configuration is now desired
truth; each server retains the provider configuration it actually applied.
An explicit pending configuration stage survives post-commit failures. Same
listener reload stops/joins the old server first; a different listener prepares
the new server before stopping the old one. Identical maintenance is a no-op;
explicit private reconfigure reloads TLS files even when paths are unchanged.
Invalid TLS input is rejected before commit or listener disruption. A failed
Store commit leaves the existing desired/applied configuration intact.

Recognized temporary listener bind errors retain pending work for normal periodic
maintenance. Startup uses this same reconciliation, so an occupied optional role
listener does not discard the persistent Endpoint. Fatal shutdown failures stop
the Runtime rather than claim a server is applied. Once installed, a server is
not restarted merely because subsequent candidate/publication work fails.
Advertisement signing is gated on matching desired/applied provider state; the
existing retained signed-batch renewal/retry implementation is unchanged.

Relay mutation replies now carry the configuration commit receipt. Every live
RequestId mutation must supply its own revision; the dispatcher no longer falls
back to a later revision read. Missing receipt leaves its reservation fail-closed.

Full Runtime testing also exposed the existing failed-initialization cleanup
race: immediate restart could encounter its previous Iroh bind port. Failed
Actor initialization now joins resources and Store before returning its original
error. Shutdown cleanup attempts every owned resource even if one close fails.
The existing startup-publication regression passes after that correction.

Workstream checks are not final acceptance: a full Runtime run passed its unit
suite but failed `full_spaces_keep_snapshot_details_and_events_within_the_frame_budget`
with `event stream closed`. Its snapshot/detail sizes were within bounds. Preserve
and diagnose this failure during the final reliability audit; do not dismiss it
based on a subsequent green run.

Private server fault scenarios: 9/9 passed, then 20 repeated runs of the built
Runtime unit-test executable passed 180/180 executions without reduced concurrency
(`/tmp/ma2a-phase1-private-repeat-final`). An earlier cargo repeat batch passed
15 rounds before a concurrently edited Store helper caused a compilation error;
that was zero test executions in round 16, not a runtime race. The final repeat
used the already built executable to isolate execution from subsequent edits.
Strict Net/Store/Runtime Clippy passed before the LOC-only module splits; repeat
strict checks after the splits. The project 250 pure-LOC limit required moving
Actor loop, Echo handle methods and response revision helpers to focused modules.

## Historical locally owned owner-invalid state

The before-fix regression `historical_owned_owner_removal_is_rejected_on_open`
failed because Repository::open accepted the adversarial signed owner-removal
chain with its original authority reference (`/tmp/ma2a-phase1-historical-before.log`).
The selected policy is repository rejection, not silent repair/quarantine metadata.
`space_rows::load_chain` checks owner membership/revocation whenever authority
custody exists. All authority signing paths load that chain, so both reopening
and continuing through an already open Repository fail closed. `replace_chain`
checks incoming chains for owned Spaces inside the transaction, covering normal
imports and control batches too. Core signed-chain rules remain unchanged.

The regression imports the adversarial chain without an authority reference,
verifies it can reopen as an external Space, then directly restores the historical
reference via test-only SQL. It proves new invitation issuance, redemption and
owned updates fail without revision or protected-key changes. Repository opening
also rejects that state. No migration, generation fabrication or key deletion is
performed. Existing affected users require explicit operator recovery; no automatic
recovery semantics are invented. Chain row derivation was moved unchanged into a
small module to retain the project's 250 pure-LOC source limit.

## Complete snapshot transfer without aggregate overflow

Confirmed before fix: `complete_large_snapshot_can_be_encoded_for_bounded_streaming`
with 256 valid escaped-name Space summaries returned INTERNAL, despite every field
and collection meeting its own bound. Separately, 300 legal Spaces could not even
construct `space_list` (INVALID_INPUT). Logs are
`/tmp/ma2a-phase1-snapshot-before.log` and `...space-list-before.log`.

The logical projection remains complete. Snapshot fetch and snapshot-derived Space
list can stream one frozen response through ordered `snapshot_fragment` frames.
Each carries at most 16,384 bytes as canonical lowercase hex, plus revision, boot,
index and final marker. The largest possible frame is below 33,000 bytes, so the
65,536-byte frame budget is preserved without smaller fabricated Space limits.
Normal responses and request/event bounds are unchanged. Aggregate snapshot arrays
and coverage are no longer capped at 256; per-entity limits and bounded observation
history are preserved. A 270-Space real IPC test passed after restart (84 seconds),
including exact Space identity inventory and revision/boot consistency.

IPC assembles only contiguous, same-revision/same-boot frames on one correlation.
HTTP sends large payloads as NDJSON with the same fragment shape. The browser
validates every fragment and the reconstructed revision before publishing any
snapshot; incomplete, reordered, mixed or duplicated streams fail. A new request
starts from a new frozen snapshot. No persistent cursor cache is introduced.
Memory for assembly remains proportional to complete state; this is bounded-frame
lossless transport, not a claim of constant-memory traversal of arbitrary state.

Both machine schema copies/hash, TypeScript models/codec/type checker, HTTP client
and protocol docs are updated together. Existing fabricated 64-Space UI capacity
meter was removed; it was not an enforced Phase-1 capacity. No business schema or
SQLite migration is added by this transfer change.

Snapshot workstream checks: seven API/fragment unit tests, one HTTP framing test,
13 contract/schema tests, and 124 Web tests passed. Four deterministic fragment
tests passed 20 repeats (80 executions). Strict Runtime all-target/all-feature
Clippy and pure-LOC checks passed. These do not replace final whole-workspace/E2E
validation. The extended real IPC test also covers the large Space list; retain
its final log separately as `/tmp/ma2a-phase1-snapshot-ipc2.log`.

## Final audit: control completion and failure cleanup

The audit reproduced two further lost-completion paths: a control round committed
membership but lost ManifestAdvanced scheduling after a candidate refresh error;
and a fatal Actor maintenance failure returned without persisting not-ready or
joining its Endpoint. Logs: `/tmp/ma2a-phase1-control-completion-before2.log` and
`/tmp/ma2a-phase1-fatal-cleanup-before.log`. An explicit membership observation
also bypassed candidate/access reconciliation (`/tmp/ma2a-observation-completion-before.log`).

Control exchanges now distinguish remote rejection from local Runtime/Store
failure. On a completed or ambiguous exchange the Actor retains completion work,
reloads membership and revision in one Store read transaction, refreshes current
lookup/candidates/access/publications, and clears completion only after success.
A task's old lookup is never installed over a newer local membership. Known
artifact changes retain their control triggers; ambiguous failures still refresh
projections, while normal periodic control performs eventual propagation. Empty
rounds with no peers produce no reconciliation work. The existing narrow
transient/fatal policy is reused; integrity, clock and task failures propagate.

Explicit owned updates and membership observations use the same retained
membership follow-up. Post-commit membership-query failures carry the committed
revision and retain Store-derived recovery work. Durable membership events occur
at commit even if completion fails. Departure errors expose a committed revision
when known; a failed transport exchange no longer asserts unchanged remote
membership. Owner enrollment/departure retains local integrity errors rather than
collapsing them into a remote denial.

Actor exit always attempts resource shutdown, observation persistence and Store
join. Runtime shutdown joins tasks even if the Actor's acknowledgement failed.
The periodic scheduling regression now waits for the actual scheduling event:
a status mailbox reply is not a maintenance barrier because commands have higher
select priority. Existing partial signed-batch tests remain unchanged.

## Final audit: read response receipts

A deterministic adapter test supplies a deliberately older dispatch status after
Space creation. The old handshake produced no authoritative receipt
(`/tmp/ma2a-read-receipt-before.log`). Handshake now reads credential state and
revision from the same frozen Store snapshot; Space show uses its snapshot
revision; control-peer status returns an Actor receipt; public/private relay
status uses a configuration/revision read transaction. Relay settings' component
rows are read in that same transaction. No wire fields change in this follow-up.

The boot-commit SQL failure regression confirms immediate restart with the same
Endpoint identity. This already passed before this audit's extra read changes;
it is coverage, not a claimed newly reproduced boot failure. Initialization's
future is boxed at its public boundary because the retained completion state
pushed existing E2E fixture futures over strict Clippy's size bound. No lint
threshold or test concurrency was changed.

Control completion/fatal cleanup/explicit observation/periodic scheduling focused
repeat: 20 rounds, 100/100 executions passed (`/tmp/ma2a-audit-repeat`).

## Final reliability audit: unnecessary Store writer reservation

The previously unresolved SSE closure was reproduced with temporary, secret-free
stage diagnostics: 16 successful executions followed by failure 17, specifically
`Repository::open -> SQLite DatabaseBusy` (`/tmp/ma2a-sse-repeat/17.log`). The
session validator closes its stream on that internal error; credentials were not
rejected. Every current-schema open unnecessarily began an IMMEDIATE migration
transaction, even though all migration steps were already applied.

`current_schema_open_reads_committed_truth_while_another_writer_is_active` holds
an independent SQLite write transaction until Repository::open returns. Baseline
fails deterministically after the existing five-second busy timeout
(`/tmp/ma2a-open-lock-before.log`). Current-schema validation now uses a DEFERRED
read transaction; actual migrations retain IMMEDIATE and recheck the schema
inside that transaction. All migration evidence, quick_check, key-reference and
owned-chain validation remains enabled. No authentication policy, cookie, TLS,
transport security, timeout or migration version changes. The companion test
rejects missing migration evidence on the read path. Diagnostic Web code was
removed after locating the failure.

The background transient-map test now drains startup control before injecting
its periodic fault. Advancing its paused clock could otherwise expire an in-flight
startup round; the new control completion reconciliation then correctly consumed
the injected fault and the periodic tick already recovered the map before the
assertion. The expected intermediate and final projections remain unchanged.
The synchronized test passed 20/20 repeats (`/tmp/ma2a-transient-repeat`).

The CLI creation integration's old assertion equated a later snapshot revision
with the earlier creation transaction. Address publication legitimately advances
Store after creation under the new receipt contract (observed 6 versus 5). The
updated test verifies the exact persisted replay response and its revision, the
same Space identity in the snapshot, and monotonic snapshot revision, while
retaining label/membership/restart assertions. It does not overwrite creation's
receipt with a later status revision to satisfy the old assertion.

The first post-writer-lock SSE stress cohort had 5 passes and then received a
legal heartbeat comment before its state notification. The test incorrectly
asserted that the first body chunk was a revisioned event. A deterministic
heartbeat-then-state stream fails that old reader
(`/tmp/ma2a-sse-heartbeat-before.log`). The integration reader now skips SSE
comments; it still requires a real invalidation/resync event within the original
five-second deadline and still fails on a closed stream or malformed UTF-8.
Production SSE and authentication behavior is unchanged. The first attempted
final stress directory (`/tmp/ma2a-sse-final`) is excluded: a build failure left
a stale binary available to that shell loop. Verified stress gates execution on
a successful build and uses `/tmp/ma2a-sse-verified`.

## Enrollment response-loss recovery

Unrestricted `cargo test -p ma2a-app` exposed a real bootstrap-header loss in
`generation_57_invite_establishes_only_after_contiguous_generation_58` (71/72 E2E
cases passed in that run; `/tmp/ma2a-final-head/ma2a-app.log`). This is distinct
from the earlier revision-baseline race. A real-QUIC deterministic handler drops
the first response after receiving the request; the old exchange fails at
bootstrap_header (`/tmp/ma2a-enrollment-response-before.log`).

Invitation redemption now opts into one exact-byte transport retry for a lost
bootstrap header/body, inside the existing total 30-second exchange deadline.
Every attempt closes its connection on failure or completion. Owner transaction
idempotency still binds ticket, candidate Endpoint and RequestId; no request is
reminted and no replay reservation is released. Explicit denial and malformed
framing are never retried. Ordinary enrollment-ALPN exchanges, including Space
departure, retain single-attempt behavior: they do not inherit this opt-in rule.
The background transient/fatal policy is unchanged. Repeated transport loss is
still a bounded, ambiguous terminal failure, not a false success.

Five Net regressions use real QUIC plus explicitly test-only framing payloads:
first-response loss with exact-byte retry, repeated loss stopping after two
attempts, malformed framing, explicit denial, and non-opted-in ambiguous exchange.
They test transport recovery; existing Core/Store enrollment and E2E replay tests
continue to verify signed bootstrap validity and single generation advancement.

## Validation follow-ups: SSE framing and control deadline

The first build-gated SSE cohort passed 12 repeats before the separate revoked
stream check failed on its first body item (`/tmp/ma2a-sse-verified/run-13.log`).
That assertion did not record the item's contents, so its cause is not asserted
from that log. The revoked-stream scenario alone then passed 30/30 diagnostic
repeats. A deterministic heartbeat-then-EOF fixture demonstrates that treating
any body item as a state event is incorrect (`/tmp/ma2a-sse-eof-before.log`).
The shared reader now skips only comments/blank frames for both state and EOF
checks. Revocation/expiry still reject every state-bearing frame and retain the
original two-second close deadline. State invalidation still requires a real
state event within five seconds. No production Web behavior changed. Verified
stress copies the successfully built binary before starting its 20 repeats;
logs are `/tmp/ma2a-sse-protocol-verified`.

The first complete package pass succeeded: Core 91, Store 58, Net 96, Runtime 167,
App 191 (603 including two doc tests). The subsequent xtask run failed the control
identity-restart scenario's explicit 15-second sync deadline: 434/435 executed
passed, with 185 not executed after fail-fast (`/tmp/ma2a-verified-head/xtask.log`).
This overlapped additional independent SSE stress work. That overlap is recorded,
not treated as proof of the failure's cause. Neither control deadlines nor normal
nextest concurrency are weakened; the final quality run and dedicated control
repeats must establish the accepted result separately.

## Consolidated implemented contract

1. Mutation admission precedes possible effects. Only proven no-effect errors may
   release it; committed/ambiguous errors are terminal or remain Pending if their
   response cannot be retained. Permanent fences outlive bounded response caching.
2. Mutation payloads and revisions travel in Store/Actor commit receipts. Read
   adapters likewise carry their own coherent snapshot/configuration revision.
3. Committed membership and enrollment effects retain follow-up work. Periodic
   reconciliation re-derives projections; exact invitation response-loss recovery
   is bounded to one retry and never re-mints an invitation or RequestId.
4. Relay configuration is durable desired truth; the live listener retains applied
   configuration. Same-listener replacement joins the old resource first; transient
   application failure stays pending, and shutdown failure is fatal.
5. Historical chains contradicting a retained local authority reference fail closed
   on load/replacement. Generic externally owned Core chain validation is unchanged.
6. Complete frozen snapshots and Space lists use bounded fragments when needed.
   Logical data is not dropped to satisfy the transport budget.

The final SSE reader passed 20 repeats of all four tests (80/80), after a successful
four-test build/run, in `/tmp/ma2a-sse-protocol-verified`. Each repeat used the same
copied binary. Net response-loss tests passed 100/100 across 20 repeats in
`/tmp/ma2a-enrollment-response-repeat`. Strict all-target/all-feature Clippy and
pure-code LOC checks passed after these changes (`/tmp/ma2a-verified-head/clippy.log`
and `loc.log`). The final three-round E2E evidence, dedicated enrollment/control
stress, and complete quality gate are recorded separately in the task report and
final-head GitHub runs; intermediate failed cohorts above are not relabeled green.

Remaining design limits are deliberate: Pending and evicted responses may be
unrecoverable and remain fail-closed; fences grow with distinct RequestIds; old
pre-migration evictions cannot be reconstructed. Pending observation/publication
batches remain process-local. Snapshot assembly is proportional to total state,
with no cross-restart streaming cursor. Historical owner-invalid repositories
require operator recovery; this pass does not repair signed chains or delete keys.
A permanent network outage or expired invitation can still leave an enrollment
exchange ambiguous, with its RequestId fenced against duplicate execution.

## Snapshot fixture I/O budget

The isolated control-restart cohort passed 20/20 without changing its 15-second
sync deadline. The next local full quality run passed that scenario but timed out
the 270-Space snapshot test at nextest's unchanged 120-second limit (456 passes,
one timeout, 164 not run; `/tmp/ma2a-verified-head/xtask-final.log`).
Stage instrumentation reproduced a 125.37-second standalone run: initial shutdown
0.84s, 270 owned Spaces persisted 44.32s, restart 88.82s, credential service opened
94.67s, snapshot fetched 106.37s. The final Space-list query and shutdown then
completed. Instrumentation was removed after locating the cost.

The snapshot fixture now retains one locally owned Space and batch-imports 269
real independently signed public genesis chains. Every Space still contains the
same local Endpoint, escaped names are unchanged, and the test retains all 270
Space identities, real SQLite persistence, Runtime restart, complete IPC snapshot
and Space-list assertions, revision/boot checks and the greater-than-one-frame
assertion. It additionally verifies all 270 durable memberships before restart.
This removes unrelated repeated private signing-key provisioning and independent
setup commits; production durability, signatures, validation, timeouts, test count
and nextest concurrency are unchanged. The standalone test passed in 83.23s
(`/tmp/ma2a-large-snapshot-batched.log`).

Final dedicated enrollment stress passed 80/80: the three original scenarios
60/60, plus long-chain enrollment 20/20 (`/tmp/ma2a-enrollment-final-stress`).
Final RequestId failure/restart/completion repeats passed 80/80 across 20 rounds
(`/tmp/ma2a-replay-final-stress`). Baseline enrollment was 60/60; this is not a claim
of a measured nonzero baseline failure rate or proof that network timeouts cannot
occur. Deterministic injected partial-commit/response-loss regressions establish
the corrected failure behavior independently of those observed stress rates.
