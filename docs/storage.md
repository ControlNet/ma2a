# Storage

MA2A uses one bundled SQLite database in each current-user state directory. The
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
4. Reject `PRAGMA user_version` values newer than schema version 11 before changing journal mode
   or running a migration.
5. Configure a five-second busy timeout, foreign-key enforcement, WAL, and `synchronous=FULL`.
6. Apply every missing migration, 1 through 11 in order, in one `BEGIN IMMEDIATE` transaction,
   validate the migration records, and run `quick_check`.
7. Resolve every Endpoint and Space-authority key reference and validate its protected file.

The store never performs an implicit downgrade or upgrades an unknown future schema.

## Schema Version 11

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

Later migrations add:

| Migration | Adds |
| --- | --- |
| `0002_web_auth.sql` | an authentication epoch for the Web password and the rebuilt session table |
| `0003_relay_persistence.sql` | Private Relay serving configuration, public fallback URLs, and served Spaces |
| `0004_local_mutation_replay.sql` | local mutation replay records, keyed by RequestId |
| `0005_relay_advertisement_activity.sql` | whether each stored Private Relay advertisement is active |
| `0006_mutation_fences.sql` | permanent RequestId fingerprints of admitted mutations (no command bodies) |
| `0007_service_exposures.sql` | file roots, per-Space service exposures, and Transfer staging records |
| `0008_processes.sql` | durable Processes and the Process exposure kind |
| `0009_agents.sql` | Agent definitions, instances, outbox, tasks, events, artifacts, tool calls, and the Agent exposure kind |
| `0010_hosted_services.sql` | hosted service registrations and the hosted-service exposure kind |
| `0011_departure_receipts.sql` | the generation that removed each departure request's sender, per Space, sender, and RequestId |

The protocol documents under [protocol/](protocol/) describe what each family stores and when it
is retained or removed.

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

SQLite refers to private keys only through the `endpoint_key_ref` and `authority_key_ref` text
columns. It never contains Endpoint private bytes, Space authority private bytes, passphrases, raw
session bearer tokens, or raw invitation tokens. It does contain other sensitive state, listed under
[Backup](#backup).

Each key reference maps to a separate immutable file below `keys/endpoint` or
`keys/space-authority`. A write creates a same-directory temporary file, applies owner-only
permissions or the explicit Windows DACL, writes and syncs the bytes, atomically renames it, and
syncs the parent directory on Unix. Rotation writes a new opaque reference rather than replacing an
existing key file. Loaded bytes are held by `ProtectedSecret` and securely zeroized on drop.

## Backup

`Repository::backup_to` uses SQLite's online backup API while the source connection remains open.
It does not copy a live database file. The backup is written to a staging file inside a new
directory beside the destination, `.ma2a-backup-<32 hex>/`, whose name does not depend on the
destination's. The directory is owner-only (mode 0700 on Unix, a current-user-and-SYSTEM ACL on
Windows), and the file is created exclusively and, on Unix, owner-only from the moment it exists,
then held open. Only after it passes `PRAGMA integrity_check = 'ok'` is it published under the
destination name by a hard link that never replaces an existing file. On Unix, the staging name and
then the destination are checked to refer to the held file (same device and inode); if either does
not, the backup fails and leaves whatever is there alone. There is no rename fallback: on a file
system without hard links the backup fails rather than risk replacing a file another backup
published meanwhile. Concurrent backups to one destination publish exactly one, and a failed backup
leaves no destination behind. It removes its staging file only while that name still refers to the
held file, so a replacement is never deleted, and the directory only once it is empty.

These checks rest on one precondition: no other account can rename entries anywhere along the
destination's path, which is how the private staging directory, or a directory above it, could be
swapped for another. On Unix the backup resolves the destination's directory and refuses it unless
every directory from the root down belongs to the current user or root and grants no group or other
write unless it is sticky (as `/tmp` is). A group-writable home directory is refused too, even for
the user's own group. On Windows those directories' ACLs are the operator's to keep. Processes of
the same account are outside this boundary: they can read the state itself.

The backup is a complete copy of the SQLite state, so it needs the same protection as the state
directory. Besides opaque key references, public chains, and digests of invitation, session, and
CSRF tokens, it carries:

- the Argon2id verifier of the Web password, which an attacker can try to guess offline;
- for every Process not yet terminal, its 32-byte supervisor ownership credential, which lets the
  holder prove itself to that Process's supervisor; it is cleared from the live row when the
  Process ends, but never from a backup taken earlier;
- the start specification of every retained Process and every Agent definition: program,
  arguments, working directory, and environment values, in decodable form, including any secret
  an operator passed that way;
- member labels, signed address records naming network addresses, relay configuration, exposures,
  and Agent task history.

It does not carry the protected-key files below `keys/`, which need a separate explicit secure
export, nor any other file: exported directories, Process output logs, Agent workspaces and
artifacts, and hosted-service files must be backed up on their own. MA2A does not implement
automatic key replication or a recovery wizard.

## Verification

Run the focused storage suite with:

```sh
cargo nextest run -p ma2a-store --all-features
```

The suite uses real SQLite files and filesystem permissions to cover migration idempotence, future
schema rejection, structural secret-column absence, rollback/reopen behavior, deterministic
concurrent redemption, owner-only key files, startup permission rejection, and backup integrity.
