use ma2a_core::{EndpointId, SpaceId};

use crate::{Repository, StoreError};

/// Public local-user-safe Space facts read from one `SQLite` snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SnapshotSpace {
    space_id: SpaceId,
    member_count: u32,
}

impl SnapshotSpace {
    /// Returns the verified Space identifier.
    pub const fn space_id(self) -> SpaceId {
        self.space_id
    }

    /// Returns the current derived member count.
    pub const fn member_count(self) -> u32 {
        self.member_count
    }
}

/// Durable state used to construct an authoritative Runtime snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapshotState {
    revision: u64,
    spaces: Vec<SnapshotSpace>,
    password_set: bool,
    active_sessions: u32,
}

impl SnapshotState {
    /// Returns the revision read in the same `SQLite` transaction as every field.
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// Returns current Spaces in which the local Endpoint is a member.
    pub fn spaces(&self) -> &[SnapshotSpace] {
        &self.spaces
    }

    /// Returns whether a UI password verifier exists.
    pub const fn password_set(&self) -> bool {
        self.password_set
    }

    /// Returns the number of unexpired, unrevoked sessions in the current auth epoch.
    pub const fn active_sessions(&self) -> u32 {
        self.active_sessions
    }
}

impl Repository {
    /// Reads local-user-safe durable snapshot state from one `SQLite` read transaction.
    ///
    /// # Errors
    /// Returns [`StoreError`] when snapshot rows are malformed or cannot be read.
    pub fn snapshot_state(
        &mut self,
        endpoint_id: EndpointId,
        now_ms: i64,
    ) -> Result<SnapshotState, StoreError> {
        let transaction = self.connection.transaction()?;
        let revision = transaction.query_row(
            "SELECT revision FROM runtime_metadata WHERE singleton = 1",
            [],
            |row| row.get(0),
        )?;
        let mut statement = transaction.prepare(
            "SELECT m.space_id, COUNT(all_members.endpoint_id)
             FROM members AS m
             JOIN members AS all_members ON all_members.space_id = m.space_id
             WHERE m.endpoint_id = ?1
             GROUP BY m.space_id
             ORDER BY m.space_id",
        )?;
        let spaces = statement
            .query_map([endpoint_id.as_bytes().as_slice()], |row| {
                Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, u32>(1)?))
            })?
            .map(|row| {
                let (space_id, member_count) = row?;
                Ok(SnapshotSpace {
                    space_id: SpaceId::try_from(space_id.as_slice()).map_err(|_| {
                        StoreError::SchemaMismatch {
                            detail: "snapshot Space identifier is invalid",
                        }
                    })?,
                    member_count,
                })
            })
            .collect::<Result<Vec<_>, StoreError>>()?;
        drop(statement);
        let password_set = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM ui_credentials WHERE singleton = 1)",
            [],
            |row| row.get(0),
        )?;
        let active_sessions = transaction.query_row(
            "SELECT COUNT(*) FROM sessions AS session
             JOIN ui_credentials AS credential ON credential.singleton = 1
             WHERE session.revoked_at_ms IS NULL
               AND session.auth_epoch = credential.auth_epoch
               AND session.idle_expires_at_ms > ?1
               AND session.absolute_expires_at_ms > ?1",
            [now_ms],
            |row| row.get(0),
        )?;
        transaction.commit()?;
        Ok(SnapshotState {
            revision,
            spaces,
            password_set,
            active_sessions,
        })
    }
}
