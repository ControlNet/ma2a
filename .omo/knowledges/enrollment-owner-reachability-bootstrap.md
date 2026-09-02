# Enrollment Owner Reachability Bootstrap

- Successful enrollment frames use the bounded `EnrollmentBootstrap` envelope: `MEB1`, a big-endian
  enrollment-page length, the existing canonical `EnrollmentPage`, a big-endian address-record
  length, and one canonical signed owner `SpaceAddressRecordV1`.
- The owner address record is required only in frame zero. Decoding rejects missing records,
  non-canonical bytes, trailing data, and frame/page/address counts or lengths beyond protocol bounds.
- Candidate validation binds the bootstrap to the invite Space and owner endpoint, verifies the owner
  signature, requires current owner membership, and applies address-record freshness rules.
- Candidate persistence uses one `ControlBatch` transaction for the complete contiguous Space chain
  and validated owner address. Invalid bootstraps leave both chain and address state unchanged.
- After persistence, the candidate publishes its own Space address and schedules an explicit control
  round scoped to the owner endpoint. This removes the previous manual address-seeding requirement.
- Exact enrollment retry rebuilds bootstrap frames from the durable chain and current persisted owner
  address. Denial status and framing remain unchanged.
- Verification on 2026-09-02: bootstrap protocol/store tests, enrollment transport tests, runtime
  scheduling tests, all enrollment integration tests, all-target checks, formatting, and strict
  Clippy passed. The cold two-runtime E2E proved owner reachability and automatic targeted sync.
- The existing `control_sync_e2e::explicit_round_converges_shared_spaces_without_leaking_private_space`
  assertion still fails because startup republishes a newer signed local address than the fixture's
  expected bytes; this belongs to the concurrent address/relay lifecycle work, not bootstrap framing.
