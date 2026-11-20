# Review round 4 — fixes and deferrals (2026-11-20)

The round-4 review (`2026-11-20-review-round-4.md`, Q01–Q28) was taken against `9914f0e`. Every
finding was re-checked against the source before fixing; all 28 held. Fixes are one commit each on
`master` after `9914f0e`.

## Status per finding

| ID | Sev. | Status | Commit / reason |
| --- | --- | --- | --- |
| Q01 | high | fixed | `f6e87c3` repeated departure answers with the chain cut at the removal |
| Q02 | high | fixed | `fb168ee` push pages carry only records the receiver's authorization accepts |
| Q03 | medium | fixed | `1c78806` signing stops at generation 239; 240–255 reserved for revocations |
| Q04 | medium | fixed | `6ecd07c` >64 shared Spaces split into several requests, interleaved per peer |
| Q05 | medium | documented | `181b0fd` limitation note; expired-record dial hint not built (relay authorization question) |
| Q06 | medium | deferred | backup race needs two concurrent public-API callers; no production caller races |
| Q07 | medium | fixed (docs) | `9e16401` backup contents listed; schema test now lists every secret column |
| Q08 | low | deferred | Windows-only ACL predicate; no native Windows test environment |
| Q09 | low | fixed | `6df24a8` SSE baseline read is re-authenticated |
| Q10 | medium | fixed | `1526afc` session checks reuse the connection verified at start |
| Q11 | medium | fixed (docs) | `712a9d3` security guide, CLI docs, and `--host` help describe plain-HTTP non-loopback mode |
| Q12 | high | fixed | `63b0183` remote Files cannot address `.ma2a-upload-*` / `.ma2a-write-*` |
| Q13 | medium | partly | `63b0183` remote callers can no longer plant staging names; a file the local operator places under such a name, and parent renames during recovery, remain |
| Q14 | high | fixed | `859d823`, `9a76073` a download takes over only helper files it can prove it wrote, including the staged sidecar |
| Q15 | medium | fixed | `022e7b0` a torn frame resets the stream instead of appending a terminal frame |
| Q16 | low | fixed (docs) | `b5fff68`, `96ab5c1` a grant replaces all eight bits, with a keep-the-rest example |
| Q17 | medium | fixed | `a1c6e6a` an unreadable status record is retried, not concluded as lost |
| Q18 | medium | fixed | `ffc43da` log retention removes front segments only after the file is gone; reads skip gaps |
| Q19 | high | fixed | `8d7cc21` an unbound Process of an undefined Agent is terminated |
| Q20 | medium | fixed | `73f8e62` SDK loop ends on output failure and drains queued lines at end of input |
| Q21 | low | fixed | `37873de` A2A timestamps reject impossible days and an empty fraction |
| Q22 | high | documented | `9b37ad3` Windows ACL caveat for custom token paths; code fix needs native Windows |
| Q23 | medium | fixed | `9b37ad3` per-process staging name; an adapter removes only its own token file |
| Q24 | medium | deferred | hosted admission across manifest relink judged practically unreachable |
| Q25 | medium | fixed | `8321d0c` an exposure that would push the list past 65,536 bytes is refused |
| Q26 | low | fixed | `7efd26c` Services polling chains each round after the previous one settles |
| Q27 | high | fixed | `9e70b5a` E2E script replaces only a directory it marked as evidence |
| Q28 | medium | fixed | `a97e8c5` RUSTSEC-2026-0543 (unmaintained chrono, transitive) acknowledged, review 2027-05-20 |

## Unconfirmed items (not counted in Q01–Q28)

Neither was reproduced, and neither was changed:

- **Mixed-length manifest paging.** The page builder skips a manifest that exceeds the remaining
  page budget and tries the next one, while the receiver requires contiguous generations. Open
  question: can normal member operations produce a chain where the first discontiguous page is
  actually sent, and does the next round then advance by itself? Needs a real signed chain that
  crosses the negotiated budget.
- **Control response write under backpressure.** The control handler holds its admission permit
  while writing the response, with no application-level deadline on that write. Open question:
  the real reclaim bound under sustained write backpressure for the supported iroh transport;
  needs a controlled-transport test before adding an overall write deadline.

## Notes worth keeping

- A Space chain holds at most 255 manifests (`MAX_SPACE_CHAIN_MANIFESTS`); export and enrollment
  paging (32 pages) both refuse more. Every production signing path goes through
  `advance_owned_space` or enrollment redemption in `ma2a-store`, so `space_capacity::admit` and
  `deliverable` there cover all of them.
- Control requests and responses are subset-safe: the responder and `apply_response` check only
  that each page's Space is shared, so splitting a peer's Spaces across requests needs no protocol
  change. The responder allows two concurrent control streams per peer; with interleaving, a third
  same-peer request only appears beyond 128 shared Spaces; whether the per-peer limit then answers
  with a transient (retried) rejection was not verified.
- An encoded `service_exposure` with a short path is about 200–250 bytes, so 256 exposures fit one
  response only with modest paths; the expose-time bound uses the widest envelope.
- `cargo test -p ma2a-app` occasionally fails in `echo_e2e` with `Address already in use`; it is
  a port race in the harness, unrelated to these changes, and passes on rerun.

## Verification at the end of the campaign

Run on `96ab5c1`, the last code change:


- `cargo build --workspace --bins` then `cargo run --locked -p xtask -- check`: exit 0, cargo-deny
  advisories/bans/licenses/sources ok.
- `(cd web && bun run check && bun run test)`: 150 passed.
- `MA2A_E2E_RUNS=1 ./scripts/run-e2e.sh`: exit 0, 72 tests, 13 evidence records.
