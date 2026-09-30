# Phase 2 caller side: CLI, refusals, acceptance E2E, and audit fixes

## CLI over the stream plane

`ma2a-app/src/commands/remote.rs::open` sends `stream_open` over IPC (the daemon must be running),
upgrades the connection with `LocalApiClient::open_stream`, then writes the `OpenHeader` frame. The
drivers live in `ma2a-services::client` (`files`, `transfer`, `process`, `proxy`, `resume`,
`terminal`). `ClientError`'s Display starts with the stable status name (for example
`unauthorized: target refused the operation: …`); E2E tests match on that name.

Exit codes of `exec`/`shell`: the remote status, 124 timeout, 130 cancel, 128+N signal N, 255
shutdown or unknown.

## Which refusal a caller sees

- An Endpoint sharing no Space with the target cannot locate it (address lookup is Space-scoped),
  so it sees `unavailable`, not `unauthorized`.
- A revoked member still resolves addresses it already holds and sees `unauthorized`.
- `service expose` into a Space the Endpoint is not in is `not_found`.

## Acceptance suite

`cargo nextest run -p ma2a-app --all-features -E 'binary(services_e2e)'` (unix only, ~40 s) runs two
real daemons per test via `tests/services_e2e/harness.rs` (`Lab::start` exposes files/exec/shell/
proxy and grants all five). Shell resize has no CLI trigger without a TTY, so that test drives the
same local API stream directly.

## Consent ends with membership (audit fix)

`ma2a-store/src/space_rows.rs::replace_chain` is the single writer of stored chains (import,
enrollment, control sync, departure). After deriving members it calls
`service_exposures::withdraw_departed`, which deletes the Space's exposures when the local Endpoint
is no longer a member, and then forgets unbound `file_roots`. The deletion is guarded by
`EXISTS (SELECT 1 FROM endpoints)` so stores without an identity (unit fixtures) are untouched.
Test: `ma2a-store/tests/exposure_withdrawal.rs`. A removal manifest must carry a signed
`SpaceRevocationV1` for the removed member, or `SpaceChain::apply` fails with `InvalidRevocation`.

## Web Services view

`GET /api/v1/services` and `GET /api/v1/operations` (authenticated reads via
`web/runtime_routes.rs::read`) back the console's Services route, which polls every 3 s while open.
Grants and consent are deliberately not routed over HTTP.

## Known load flake

`e2e space_departure::*` can fail with `SpaceDepartureError(Unreachable)` under full-workspace load
(seen once in 717 tests). It passes alone and in the next full run. The departure code is unchanged
by Phase 2.
