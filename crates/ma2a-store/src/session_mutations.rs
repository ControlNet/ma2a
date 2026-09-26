use rusqlite::OptionalExtension as _;
use subtle::ConstantTimeEq as _;

use crate::{
    Repository, SessionAdmission, SessionCreate, SessionDigests, SessionRecord, SessionTouch,
    StoreError,
    repository::increment_revision,
    sessions::{insert_session, read_session},
};

#[derive(Clone, Copy)]
enum SessionAuthentication<'a> {
    Bearer(&'a [u8; 32]),
    BearerAndCsrf(&'a SessionDigests),
}

impl Repository {
    /// Validates one session without extending its deadlines or advancing revision.
    ///
    /// # Errors
    /// Returns [`StoreError`] when authentication state cannot be read.
    pub fn validate_session(
        &self,
        session_id_hash: &[u8; 32],
        now_ms: i64,
    ) -> Result<Option<SessionRecord>, StoreError> {
        let Some(session) = read_session(&self.connection, session_id_hash)? else {
            return Ok(None);
        };
        let auth_epoch = self
            .connection
            .query_row(
                "SELECT auth_epoch FROM ui_credentials WHERE singleton = 1",
                [],
                |row| row.get::<_, u64>(0),
            )
            .optional()?;
        if session.revoked_at_ms.is_some()
            || auth_epoch != Some(session.auth_epoch)
            || now_ms >= session.idle_expires_at_ms
            || now_ms >= session.absolute_expires_at_ms
        {
            Ok(None)
        } else {
            Ok(Some(session))
        }
    }

    /// Inserts a session only while its credential epoch is current and capacity remains.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the atomic decision cannot be committed.
    pub fn create_session_if_current(
        &mut self,
        session: &SessionRecord,
        admission: SessionAdmission,
    ) -> Result<SessionCreate, StoreError> {
        let transaction = self.immediate()?;
        let auth_epoch = transaction
            .query_row(
                "SELECT auth_epoch FROM ui_credentials WHERE singleton = 1",
                [],
                |row| row.get::<_, u64>(0),
            )
            .optional()?;
        if auth_epoch != Some(session.auth_epoch) {
            return Ok(SessionCreate::StaleCredential);
        }
        let removed = transaction.execute(
            "DELETE FROM sessions WHERE revoked_at_ms IS NOT NULL
             OR idle_expires_at_ms <= ?1 OR absolute_expires_at_ms <= ?1",
            [admission.now_ms],
        )?;
        let active = transaction.query_row("SELECT COUNT(*) FROM sessions", [], |row| {
            row.get::<_, u32>(0)
        })?;
        if active >= admission.max_sessions {
            if removed > 0 {
                increment_revision(&transaction)?;
                transaction.commit()?;
            }
            return Ok(SessionCreate::LimitReached);
        }
        insert_session(&transaction, session)?;
        increment_revision(&transaction)?;
        transaction.commit()?;
        Ok(SessionCreate::Created)
    }

    /// Validates and monotonically touches one session in a single transaction.
    ///
    /// # Errors
    /// Returns [`StoreError`] when authentication state cannot be read or committed.
    pub fn authenticate_and_touch_session(
        &mut self,
        session_id_hash: &[u8; 32],
        touch: SessionTouch,
    ) -> Result<Option<SessionRecord>, StoreError> {
        self.authenticate_and_touch_session_inner(
            SessionAuthentication::Bearer(session_id_hash),
            touch,
        )
    }

    /// Validates a bearer and CSRF digest before monotonically touching one session.
    ///
    /// # Errors
    /// Returns [`StoreError`] when authentication state cannot be read or committed.
    pub fn authenticate_and_touch_session_with_csrf(
        &mut self,
        digests: &SessionDigests,
        touch: SessionTouch,
    ) -> Result<Option<SessionRecord>, StoreError> {
        self.authenticate_and_touch_session_inner(
            SessionAuthentication::BearerAndCsrf(digests),
            touch,
        )
    }

    fn authenticate_and_touch_session_inner(
        &mut self,
        authentication: SessionAuthentication<'_>,
        touch: SessionTouch,
    ) -> Result<Option<SessionRecord>, StoreError> {
        let (session_id_hash, csrf_token_hash) = match authentication {
            SessionAuthentication::Bearer(session_id_hash) => (session_id_hash, None),
            SessionAuthentication::BearerAndCsrf(digests) => {
                (digests.bearer_digest(), Some(digests.csrf_digest()))
            }
        };
        let transaction = self.immediate()?;
        let Some(mut session) = read_session(&transaction, session_id_hash)? else {
            return Ok(None);
        };
        let auth_epoch = transaction
            .query_row(
                "SELECT auth_epoch FROM ui_credentials WHERE singleton = 1",
                [],
                |row| row.get::<_, u64>(0),
            )
            .optional()?;
        if session.revoked_at_ms.is_some()
            || auth_epoch != Some(session.auth_epoch)
            || touch.now_ms >= session.idle_expires_at_ms
            || touch.now_ms >= session.absolute_expires_at_ms
            || csrf_token_hash
                .is_some_and(|provided| !bool::from(session.csrf_token_hash.ct_eq(provided)))
        {
            return Ok(None);
        }
        let last_seen_at_ms = session.last_seen_at_ms.max(touch.now_ms);
        let idle_expires_at_ms = session.idle_expires_at_ms.max(
            touch
                .now_ms
                .saturating_add(touch.idle_timeout_ms)
                .min(session.absolute_expires_at_ms),
        );
        if last_seen_at_ms != session.last_seen_at_ms
            || idle_expires_at_ms != session.idle_expires_at_ms
        {
            transaction.execute(
                "UPDATE sessions SET last_seen_at_ms = ?2, idle_expires_at_ms = ?3
                 WHERE session_id_hash = ?1 AND revoked_at_ms IS NULL",
                (
                    session_id_hash.as_slice(),
                    last_seen_at_ms,
                    idle_expires_at_ms,
                ),
            )?;
            // Sliding a session deliberately does not advance the Runtime revision.
            // Neither `last_seen_at_ms` nor `idle_expires_at_ms` reaches any snapshot
            // projection, so a touch is not state the console can observe. Advancing
            // for it would make every authenticated read a state change: the console
            // reads the snapshot, the read slides its own session, the new revision
            // is published as an invalidation, and the console reads again. Session
            // creation, deletion and revocation still advance, because they change
            // the `active_sessions` count the snapshot does carry.
            session.last_seen_at_ms = last_seen_at_ms;
            session.idle_expires_at_ms = idle_expires_at_ms;
            transaction.commit()?;
        }
        Ok(Some(session))
    }

    /// Deletes one session and advances revision when a row existed.
    ///
    /// # Errors
    /// Returns [`StoreError`] when deletion cannot be committed.
    pub fn delete_session(&mut self, session_id_hash: &[u8; 32]) -> Result<(), StoreError> {
        let transaction = self.immediate()?;
        let deleted = transaction.execute(
            "DELETE FROM sessions WHERE session_id_hash = ?1",
            [session_id_hash.as_slice()],
        )?;
        if deleted > 0 {
            increment_revision(&transaction)?;
            transaction.commit()?;
        }
        Ok(())
    }

    /// Revokes every active session and advances revision once when state changed.
    ///
    /// # Errors
    /// Returns [`StoreError`] when revocation cannot be committed.
    pub fn revoke_all_sessions(&mut self, now_ms: i64) -> Result<(), StoreError> {
        self.revoke_all_sessions_committed(now_ms).map(|_| ())
    }

    /// Revokes sessions and returns password presence at the same transaction revision.
    ///
    /// # Errors
    /// Returns an error when session or credential state cannot be persisted or read.
    pub fn revoke_all_sessions_committed(
        &mut self,
        now_ms: i64,
    ) -> Result<crate::Committed<bool>, StoreError> {
        let transaction = self.immediate()?;
        let changed = transaction.execute(
            "UPDATE sessions SET revoked_at_ms = ?1 WHERE revoked_at_ms IS NULL",
            [now_ms],
        )?;
        let revision = if changed > 0 {
            increment_revision(&transaction)?
        } else {
            transaction.query_row(
                "SELECT revision FROM runtime_metadata WHERE singleton = 1",
                [],
                |row| row.get(0),
            )?
        };
        let password_set = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM ui_credentials WHERE singleton = 1)",
            [],
            |row| row.get(0),
        )?;
        transaction.commit()?;
        Ok(crate::Committed::new(revision, password_set))
    }
}
