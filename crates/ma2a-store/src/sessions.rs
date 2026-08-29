use rusqlite::OptionalExtension as _;

use crate::{Repository, StoreError, repository::increment_revision};

/// Persisted session metadata represented only by domain-separated digests.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct SessionRecord {
    /// Digest of the bearer token.
    pub(crate) session_id_hash: [u8; 32],
    /// Digest of the CSRF token.
    pub(crate) csrf_token_hash: [u8; 32],
    /// Credential epoch accepted by this session.
    pub(crate) auth_epoch: u64,
    /// Session creation time.
    pub(crate) created_at_ms: i64,
    /// Most recent accepted request time.
    pub(crate) last_seen_at_ms: i64,
    /// Sliding idle deadline.
    pub(crate) idle_expires_at_ms: i64,
    /// Non-renewable absolute deadline.
    pub(crate) absolute_expires_at_ms: i64,
    /// Revocation timestamp, when revoked.
    pub(crate) revoked_at_ms: Option<i64>,
}

/// Domain-separated bearer and CSRF digests for one session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionDigests {
    bearer: [u8; 32],
    csrf: [u8; 32],
}

impl SessionDigests {
    /// Creates a pair of non-recoverable session digests.
    pub const fn new(bearer: [u8; 32], csrf: [u8; 32]) -> Self {
        Self { bearer, csrf }
    }

    /// Returns the bearer digest.
    pub const fn bearer_digest(&self) -> &[u8; 32] {
        &self.bearer
    }

    /// Returns the CSRF digest.
    pub const fn csrf_digest(&self) -> &[u8; 32] {
        &self.csrf
    }
}

/// Creation, activity, and expiry timestamps for one session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionTimestamps {
    activity: [i64; 2],
    deadlines: [i64; 2],
}

impl SessionTimestamps {
    /// Creates timestamps from activity and deadline pairs.
    pub const fn new(activity: [i64; 2], deadlines: [i64; 2]) -> Self {
        Self {
            activity,
            deadlines,
        }
    }
}

/// Atomic session capacity decision inputs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionAdmission {
    pub(crate) max_sessions: u32,
    pub(crate) now_ms: i64,
}

impl SessionAdmission {
    /// Creates bounded session admission inputs.
    pub const fn new(max_sessions: u32, now_ms: i64) -> Self {
        Self {
            max_sessions,
            now_ms,
        }
    }
}

/// Atomic session authentication touch inputs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionTouch {
    pub(crate) now_ms: i64,
    pub(crate) idle_timeout_ms: i64,
}

impl SessionTouch {
    /// Creates monotonic session touch inputs.
    pub const fn new(now_ms: i64, idle_timeout_ms: i64) -> Self {
        Self {
            now_ms,
            idle_timeout_ms,
        }
    }
}

impl SessionRecord {
    /// Creates an active session record.
    pub const fn new(
        digests: SessionDigests,
        auth_epoch: u64,
        timestamps: SessionTimestamps,
    ) -> Self {
        Self {
            session_id_hash: digests.bearer,
            csrf_token_hash: digests.csrf,
            auth_epoch,
            created_at_ms: timestamps.activity[0],
            last_seen_at_ms: timestamps.activity[1],
            idle_expires_at_ms: timestamps.deadlines[0],
            absolute_expires_at_ms: timestamps.deadlines[1],
            revoked_at_ms: None,
        }
    }

    /// Returns the bearer digest.
    pub const fn bearer_digest(&self) -> &[u8; 32] {
        &self.session_id_hash
    }

    /// Returns the CSRF digest.
    pub const fn csrf_digest(&self) -> &[u8; 32] {
        &self.csrf_token_hash
    }
}

/// Outcome of an epoch-checked, capacity-bounded session insertion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SessionCreate {
    /// The session was committed.
    Created,
    /// The credential epoch changed before insertion.
    StaleCredential,
    /// The active session limit was already reached.
    LimitReached,
}

impl std::fmt::Debug for SessionRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SessionRecord")
            .field("session_id_hash", &"[REDACTED]")
            .field("csrf_token_hash", &"[REDACTED]")
            .field("auth_epoch", &self.auth_epoch)
            .field("created_at_ms", &self.created_at_ms)
            .field("last_seen_at_ms", &self.last_seen_at_ms)
            .field("idle_expires_at_ms", &self.idle_expires_at_ms)
            .field("absolute_expires_at_ms", &self.absolute_expires_at_ms)
            .field("revoked_at_ms", &self.revoked_at_ms)
            .finish()
    }
}

impl Repository {
    /// Stores a session record for store-level transaction fixtures.
    ///
    /// # Errors
    /// Returns [`StoreError`] when session state cannot be committed.
    pub fn create_session(&mut self, session: &SessionRecord) -> Result<u64, StoreError> {
        let transaction = self.immediate()?;
        insert_session(&transaction, session)?;
        let revision = increment_revision(&transaction)?;
        transaction.commit()?;
        Ok(revision)
    }

    /// Reads one session by bearer digest.
    ///
    /// # Errors
    /// Returns [`StoreError`] when session state cannot be read or decoded.
    pub fn session(&self, session_id_hash: &[u8; 32]) -> Result<Option<SessionRecord>, StoreError> {
        read_session(&self.connection, session_id_hash)
    }
}

pub(crate) fn insert_session(
    connection: &rusqlite::Connection,
    session: &SessionRecord,
) -> Result<(), StoreError> {
    connection.execute(
        "INSERT INTO sessions(
            session_id_hash, csrf_token_hash, auth_epoch, created_at_ms, last_seen_at_ms,
            idle_expires_at_ms, absolute_expires_at_ms, revoked_at_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        (
            session.session_id_hash.as_slice(),
            session.csrf_token_hash.as_slice(),
            session.auth_epoch,
            session.created_at_ms,
            session.last_seen_at_ms,
            session.idle_expires_at_ms,
            session.absolute_expires_at_ms,
            session.revoked_at_ms,
        ),
    )?;
    Ok(())
}

pub(crate) fn read_session(
    connection: &rusqlite::Connection,
    session_id_hash: &[u8; 32],
) -> Result<Option<SessionRecord>, StoreError> {
    let row = connection
        .query_row(
            "SELECT session_id_hash, csrf_token_hash, auth_epoch, created_at_ms,
                    last_seen_at_ms, idle_expires_at_ms, absolute_expires_at_ms, revoked_at_ms
             FROM sessions WHERE session_id_hash = ?1",
            [session_id_hash.as_slice()],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, u64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, Option<i64>>(7)?,
                ))
            },
        )
        .optional()?;
    row.map(|record| {
        Ok(SessionRecord {
            session_id_hash: record
                .0
                .try_into()
                .map_err(|_| StoreError::InvalidAuthState)?,
            csrf_token_hash: record
                .1
                .try_into()
                .map_err(|_| StoreError::InvalidAuthState)?,
            auth_epoch: record.2,
            created_at_ms: record.3,
            last_seen_at_ms: record.4,
            idle_expires_at_ms: record.5,
            absolute_expires_at_ms: record.6,
            revoked_at_ms: record.7,
        })
    })
    .transpose()
}
