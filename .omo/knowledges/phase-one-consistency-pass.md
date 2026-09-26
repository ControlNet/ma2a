# Phase One consistency pass (baseline 2b6ee097)

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
