use std::fmt::Write as _;

use ma2a_core::{Capability, EndpointId, SpaceChain, SpaceId};

use crate::{Repository, StoreError};

/// Reads the shared Space name, falling back to the canonical Space identifier
/// for Spaces whose genesis predates persisted names.
fn space_name(chain: &SpaceChain) -> String {
    if let Some(name) = chain.genesis().name() {
        return name.to_owned();
    }
    let mut fallback = String::with_capacity(64);
    for byte in chain.space_id().as_bytes() {
        let _written = write!(&mut fallback, "{byte:02x}");
    }
    fallback
}

/// One signed member of a Space, as the owner's own chain already records it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapshotMember {
    endpoint_id: EndpointId,
    label: String,
    echo: bool,
    relay_provider: bool,
}

impl SnapshotMember {
    /// Returns the member Endpoint identity.
    pub const fn endpoint_id(&self) -> EndpointId {
        self.endpoint_id
    }

    /// Returns the signed member label.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Returns whether the Space policy and this member grant Echo.
    pub const fn echo(&self) -> bool {
        self.echo
    }

    /// Returns whether this member may provide private relay service.
    pub const fn relay_provider(&self) -> bool {
        self.relay_provider
    }
}

/// Public local-user-safe Space facts read from one `SQLite` snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapshotSpace {
    space_id: SpaceId,
    name: String,
    member_count: u32,
    generation: u64,
    chain_hash: [u8; 32],
    revoked_count: u32,
}

impl SnapshotSpace {
    /// Returns the verified Space identifier.
    pub const fn space_id(&self) -> SpaceId {
        self.space_id
    }

    /// Returns the shared Space name signed into genesis, or the canonical
    /// Space identifier for legacy Spaces created before names were persisted.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the current derived member count.
    pub const fn member_count(&self) -> u32 {
        self.member_count
    }

    /// Returns the latest applied manifest generation, or zero at genesis.
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Returns the latest accepted contiguous chain hash.
    pub const fn chain_hash(&self) -> [u8; 32] {
        self.chain_hash
    }

    /// Returns the number of Space-local revocations carried forward.
    pub const fn revoked_count(&self) -> u32 {
        self.revoked_count
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
                let space_id = SpaceId::try_from(space_id.as_slice()).map_err(|_| {
                    StoreError::SchemaMismatch {
                        detail: "snapshot Space identifier is invalid",
                    }
                })?;
                let chain = crate::space_rows::load_chain(&transaction, space_id)?.ok_or(
                    StoreError::SchemaMismatch {
                        detail: "snapshot Space chain is missing",
                    },
                )?;
                if !chain
                    .members()
                    .iter()
                    .any(|member| member.endpoint_id() == endpoint_id)
                {
                    return Err(StoreError::SchemaMismatch {
                        detail: "snapshot local Space member is missing",
                    });
                }
                Ok(SnapshotSpace {
                    space_id,
                    name: space_name(&chain),
                    member_count,
                    generation: chain.latest_generation(),
                    chain_hash: chain.latest_hash(),
                    revoked_count: u32::try_from(chain.revocations().len()).unwrap_or(u32::MAX),
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

/// Complete members and their chain head, read in one transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct SpaceDetails {
    /// Revision of this read transaction.
    pub revision: u64,
    /// Summary corresponding to the member set.
    pub space: SnapshotSpace,
    /// Complete signed member set.
    pub members: Vec<SnapshotMember>,
}

impl Repository {
    /// Reads one Space only if the local Endpoint is still a signed member.
    ///
    /// # Errors
    /// Returns a store error if the persisted chain is invalid or unreadable.
    pub fn space_details(
        &mut self,
        endpoint_id: EndpointId,
        space_id: SpaceId,
    ) -> Result<Option<SpaceDetails>, StoreError> {
        let transaction = self.connection.transaction()?;
        let revision = transaction.query_row(
            "SELECT revision FROM runtime_metadata WHERE singleton = 1",
            [],
            |row| row.get(0),
        )?;
        let Some(chain) = crate::space_rows::load_chain(&transaction, space_id)? else {
            return Ok(None);
        };
        if !chain
            .members()
            .iter()
            .any(|member| member.endpoint_id() == endpoint_id)
        {
            return Ok(None);
        }
        let members = chain
            .members()
            .iter()
            .map(|member| SnapshotMember {
                endpoint_id: member.endpoint_id(),
                label: member.label().to_owned(),
                echo: member.capabilities().allows(Capability::ECHO),
                relay_provider: member
                    .capabilities()
                    .allows(Capability::PRIVATE_RELAY_PROVIDER),
            })
            .collect();

        let space = SnapshotSpace {
            space_id,
            name: space_name(&chain),
            member_count: u32::try_from(chain.members().len()).unwrap_or(u32::MAX),
            generation: chain.latest_generation(),
            chain_hash: chain.latest_hash(),
            revoked_count: u32::try_from(chain.revocations().len()).unwrap_or(u32::MAX),
        };
        transaction.commit()?;
        Ok(Some(SpaceDetails {
            revision,
            space,
            members,
        }))
    }
}
