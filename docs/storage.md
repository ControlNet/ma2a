# Storage

MA2A Phase 1 uses one bundled SQLite database in each current-user state directory. The
`ma2a-store` API is intentionally synchronous: a Runtime must own it from a dedicated blocking
actor or call it through an explicit blocking boundary. It must not run database or protected-key
filesystem work directly on an async worker.

## Startup Gate

`Repository::open` performs these checks before state is available:

1. Require an absolute local state path. Windows UNC, verbatim UNC, and device paths are rejected.
2. Require current-user ownership and exact `0700` directory modes and `0600` database/key-file
   modes on Unix. Symlinks and unexpected file types are rejected.
3. Require protected Windows DACLs with exactly the current user and SYSTEM holding explicit full
   control. Inherited, broad, or remote access is rejected.
4. Reject `PRAGMA user_version` values newer than schema version 1 before changing journal mode or
   running a migration.
5. Configure a five-second busy timeout, foreign-key enforcement, WAL, and `synchronous=FULL`.
6. Apply migration 1 in `BEGIN IMMEDIATE`, validate its migration record, and run `quick_check`.
7. Resolve every Endpoint and Space-authority key reference and validate its protected file.

The store never performs an implicit downgrade or upgrades an unknown future schema.

## Schema Version 1

Migration `0001_init.sql` stores:

- Runtime boot/shutdown metadata and the monotonic state revision.
- The one public Endpoint ID and an opaque Endpoint key reference.
- Space IDs, canonical genesis bytes, and optional opaque authority-key references.
- The complete accepted contiguous signed manifest chain plus latest generation/hash state.
- Current members and signed revocations.
- Pending, consumed, revoked, and expired invitation lifecycle state and token hashes.
- Only the highest accepted current address record and private-relay advertisement per scoped
  Endpoint, including expiry and signed bytes.
- Desired relay configuration and expiring observed relay state.
- The Argon2id password verifier and revocable sessions identified only by bearer-token hashes.

Foreign keys, strict tables, checks, unique constraints, partial expiry indexes, and manifest
generation/hash indexes enforce the storage boundary. Contiguity and monotonic sequence decisions
are additionally checked by typed `BEGIN IMMEDIATE` repository operations.

## Transaction Boundaries

Invitation redemption, invitation expiry invalidation, manifest advancement, address advancement,
relay-advertisement advancement, password reset with session revocation, and explicit revision
advancement each commit as one immediate transaction. Conflict and stale outcomes return without
committing a partial unit. Dropping a transaction or connection rolls back uncommitted changes.

Every durable mutation advances `runtime_metadata.revision` in the same transaction as its state
change. Callers may use that revision as the source for snapshots and change notifications without
treating SQLite as an event log.

## Protected Keys

SQLite contains only the `endpoint_key_ref` and `authority_key_ref` text columns. It must never
contain Endpoint private bytes, Space authority private bytes, passphrases, raw session bearer
tokens, or recoverable secret material.

Each key reference maps to a separate immutable file below `keys/endpoint` or
`keys/space-authority`. A write creates a same-directory temporary file, applies owner-only
permissions or the explicit Windows DACL, writes and syncs the bytes, atomically renames it, and
syncs the parent directory on Unix. Rotation writes a new opaque reference rather than replacing an
existing key file. Loaded bytes are held by `ProtectedSecret` and securely zeroized on drop.

## Backup

`Repository::backup_to` uses SQLite's online backup API while the source connection remains open.
It does not copy a live database file. The destination must not already exist, receives protected
file permissions, and must pass `PRAGMA integrity_check = 'ok'` before the call succeeds.

The backup contains only SQLite state and opaque key references. Protected-key files require a
separate explicit secure export process; Phase 1 does not implement automatic key replication or a
recovery wizard.

## Verification

Run the focused storage suite with:

```sh
cargo nextest run -p ma2a-store --all-features
```

The suite uses real SQLite files and filesystem permissions to cover migration idempotence, future
schema rejection, structural secret-column absence, rollback/reopen behavior, deterministic
concurrent redemption, owner-only key files, startup permission rejection, and backup integrity.
