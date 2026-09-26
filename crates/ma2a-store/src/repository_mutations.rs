use rusqlite::OptionalExtension as _;

use crate::{
    InvitationRecord, ManifestAdvance, ManifestOutcome, Redemption, RedemptionOutcome, Repository,
    StoreError, repository::increment_revision,
};

impl Repository {
    /// Cancels a pending invitation and advances the repository revision.
    ///
    /// # Errors
    /// Returns an error when the invitation is not pending or the transaction cannot commit.
    pub fn cancel_invitation(&mut self, invitation_id: [u8; 16]) -> Result<u64, StoreError> {
        let transaction = self.immediate()?;
        let changed = transaction.execute(
            "UPDATE invitations SET status = 2 WHERE invitation_id = ?1 AND status = 0",
            [invitation_id.as_slice()],
        )?;
        if changed != 1 {
            return Err(StoreError::SchemaMismatch {
                detail: "pending invitation not found",
            });
        }
        let revision = increment_revision(&transaction)?;
        transaction.commit()?;
        Ok(revision)
    }

    /// Creates a pending invitation and advances the Runtime revision atomically.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the transaction cannot be committed.
    pub fn create_invitation(&mut self, invitation: &InvitationRecord) -> Result<u64, StoreError> {
        Ok(self
            .create_invitation_projected(invitation, |_| Ok(()))?
            .revision())
    }

    pub(crate) fn create_invitation_projected<T>(
        &mut self,
        invitation: &InvitationRecord,
        project: impl FnOnce(&rusqlite::Transaction<'_>) -> Result<T, StoreError>,
    ) -> Result<crate::Committed<T>, StoreError> {
        let transaction = self.immediate()?;
        transaction.execute(
            "INSERT INTO invitations(
                invitation_id, space_id, token_hash, creator_endpoint_id,
                created_at_ms, expires_at_ms, owner_bootstrap
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            (
                invitation.invitation_id.as_slice(),
                invitation.space_id.as_bytes().as_slice(),
                invitation.token_hash.as_slice(),
                invitation.creator_endpoint_id.as_bytes().as_slice(),
                invitation.created_at_ms,
                invitation.expires_at_ms,
                invitation.owner_bootstrap.as_slice(),
            ),
        )?;
        let revision = increment_revision(&transaction)?;
        let value = project(&transaction)?;
        transaction.commit()?;
        Ok(crate::Committed::new(revision, value))
    }

    /// Consumes one pending invitation exactly once under an immediate transaction.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when invitation state cannot be read or updated.
    pub fn redeem_invitation(
        &mut self,
        redemption: &Redemption,
    ) -> Result<RedemptionOutcome, StoreError> {
        let transaction = self.immediate()?;
        let invitation = transaction
            .query_row(
                "SELECT invitation_id, status, expires_at_ms FROM invitations WHERE token_hash = ?1",
                [redemption.token_hash.as_slice()],
                |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, u8>(1)?, row.get::<_, i64>(2)?)),
            )
            .optional()?;
        let Some((invitation_id, status, expires_at_ms)) = invitation else {
            let consumed = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM consumed_invitation_tokens WHERE token_hash = ?1)",
                [redemption.token_hash.as_slice()],
                |row| row.get::<_, bool>(0),
            )?;
            return Ok(if consumed {
                RedemptionOutcome::AlreadyConsumed
            } else {
                RedemptionOutcome::NotFound
            });
        };
        match status {
            1 => return Ok(RedemptionOutcome::AlreadyConsumed),
            2 => return Ok(RedemptionOutcome::Revoked),
            3 => return Ok(RedemptionOutcome::Expired),
            _ if expires_at_ms <= redemption.now_ms => {
                transaction.execute(
                    "UPDATE invitations SET status = 3 WHERE invitation_id = ?1 AND status = 0",
                    [invitation_id.as_slice()],
                )?;
                increment_revision(&transaction)?;
                transaction.commit()?;
                return Ok(RedemptionOutcome::Expired);
            }
            _ => {}
        }
        transaction.execute(
            "UPDATE invitations SET status = 1, consumed_at_ms = ?1, consumed_by_endpoint_id = ?2
             WHERE invitation_id = ?3 AND status = 0",
            (
                redemption.consumed_at_ms,
                redemption.endpoint_id.as_bytes().as_slice(),
                invitation_id.as_slice(),
            ),
        )?;
        transaction.execute(
            "INSERT INTO consumed_invitation_tokens(token_hash, invitation_id, consumed_at_ms)
             VALUES (?1, ?2, ?3)",
            (
                redemption.token_hash.as_slice(),
                invitation_id.as_slice(),
                redemption.consumed_at_ms,
            ),
        )?;
        let revision = increment_revision(&transaction)?;
        transaction.commit()?;
        Ok(RedemptionOutcome::Redeemed { revision })
    }

    /// Appends only the next contiguous signed manifest and updates latest state atomically.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when manifest state cannot be read or committed.
    pub fn advance_manifest(
        &mut self,
        advance: &ManifestAdvance,
    ) -> Result<ManifestOutcome, StoreError> {
        let Some(mut chain) = self.load_space_chain(advance.space_id)? else {
            return Ok(ManifestOutcome::Conflict {
                current_generation: None,
            });
        };
        let current_generation = chain.latest_generation();
        let manifest = ma2a_core::SignedSpaceManifestV1::from_canonical_bytes(
            &advance.signed_manifest,
            chain.genesis().authority(),
        )
        .map_err(|_| StoreError::Manifest(ma2a_core::ManifestError::INVALID_SIGNATURE))?;
        if manifest.manifest().space_id() != advance.space_id
            || manifest.generation() != advance.generation
            || advance.previous_hash != Some(manifest.previous_hash())
            || manifest.manifest_hash() != advance.manifest_hash
        {
            return Err(StoreError::SchemaMismatch {
                detail: "manifest advance metadata does not match signed bytes",
            });
        }
        if chain.apply(&manifest).is_err() {
            return Ok(ManifestOutcome::Conflict {
                current_generation: Some(current_generation),
            });
        }
        let accepted_generation = chain.latest_generation();
        let persistence = self.persist_space_chain(&chain)?;
        if persistence.error().is_some() {
            return Ok(ManifestOutcome::Conflict {
                current_generation: Some(current_generation),
            });
        }
        Ok(persistence.revision().map_or(
            ManifestOutcome::Idempotent {
                current_generation: accepted_generation,
            },
            |revision| ManifestOutcome::Advanced { revision },
        ))
    }

    /// Advances the Runtime revision as its own explicit transaction.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when runtime metadata cannot be committed.
    pub fn advance_revision(&mut self) -> Result<u64, StoreError> {
        let transaction = self.immediate()?;
        let revision = increment_revision(&transaction)?;
        transaction.commit()?;
        Ok(revision)
    }
}
