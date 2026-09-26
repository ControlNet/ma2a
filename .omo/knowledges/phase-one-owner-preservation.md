# Phase One owner preservation

Phase 1 has one Space authority and no authority transfer. The owner Endpoint is
`chain.genesis().genesis().initial_member().endpoint_id()`, independent of current
member ordering or derived role rows.

`Repository::advance_owned_space()` checks the complete proposed membership before
loading the authority secret or signing: the genesis owner must be present in
members and absent from revocations. Rejection is
`StoreError::SpaceOwnerCannotBeRemoved` and commits no generation, hash, revision,
membership, revocation, or authority-reference change.

The Runtime member-removal backend also rejects the genesis owner before building
a removal proposal. Its typed Runtime classification is
`SPACE_OWNER_CANNOT_BE_REMOVED`. The shared CLI/Web command executor maps it to the
existing `unauthorized` API code with remediation
`Space owner cannot be removed in Phase 1`. The existing `OWNER_CANNOT_LEAVE`
departure path remains separate. A rejected removal never reaches Actor projection
updates, membership events, or relay/control follow-up.

Generic Core signed-chain validation and public chain persistence remain unchanged.
The adversarial Store regression constructs an externally signed owner removal and
proves it can still be persisted and reopened, including with an existing authority
reference. Thus pre-existing owner-removed repositories are not repaired or rejected
by this patch. Deciding how to detect/recover those repositories is a separate task.

Regression coverage exercises direct owned signing at generations 1, 2 and 3,
independent omission/revocation proposals, unchanged protected key/reference and
SQLite-derived rows, Runtime event/scheduling stability, CLI and same-request-ID
IPC rejection, and ordinary member removal surviving daemon restart. The original
owner-removal regression failed on `9b4c53e` because signing succeeded.
