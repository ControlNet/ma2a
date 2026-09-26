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
