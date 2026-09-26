use iroh_base::EndpointAddr;
use ma2a_core::{
    EndpointId, InviteEntropy, InviteValidity, MemberCapabilities, SignedInviteTicket,
    SpaceAuthoritySecret, SpaceId, SpaceManifestLink, SpaceManifestMembership, SpaceManifestV1,
    SpaceMemberV1,
};
use rusqlite::OptionalExtension as _;

use crate::{
    AuthorizedEnrollmentRedemption, EnrollmentOutcome, KeyKind, KeyReference, Repository,
    StoreError,
    repository::increment_revision,
    space_rows::{load_chain, replace_chain},
};

/// Signed invitation paired with the revision committed atomically with it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreatedEnrollmentInvite {
    revision: u64,
    ticket: SignedInviteTicket,
    chain: ma2a_core::SpaceChain,
}

impl CreatedEnrollmentInvite {
    /// Returns the committed repository revision.
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// Returns the Space chain observed inside the invitation transaction.
    pub const fn chain(&self) -> &ma2a_core::SpaceChain {
        &self.chain
    }

    /// Returns the signed single-use invitation.
    pub const fn ticket(&self) -> &SignedInviteTicket {
        &self.ticket
    }
}

impl Repository {
    /// Signs and durably records a single-use enrollment invitation.
    ///
    /// # Errors
    ///
    /// Returns an error when the Space authority is unavailable or persistence fails.
    #[allow(
        clippy::too_many_arguments,
        reason = "each signed invitation field remains explicit at the repository boundary"
    )]
    pub fn create_enrollment_invite(
        &mut self,
        space_id: SpaceId,
        creator: EndpointId,
        owner_addr: EndpointAddr,
        validity: InviteValidity,
        entropy: &InviteEntropy,
    ) -> Result<CreatedEnrollmentInvite, StoreError> {
        let chain = self
            .load_space_chain(space_id)?
            .ok_or(StoreError::SpaceNotFound)?;
        let reference = self
            .connection
            .query_row(
                "SELECT authority_key_ref FROM spaces WHERE space_id = ?1",
                [space_id.as_bytes().as_slice()],
                |row| row.get::<_, Option<String>>(0),
            )?
            .ok_or(StoreError::SpaceAuthorityUnavailable)
            .and_then(|value| KeyReference::parse(&value))?;
        let protected = self.key_store.read(KeyKind::SpaceAuthority, &reference)?;
        let secret = SpaceAuthoritySecret::try_from_bytes(protected.as_bytes())?;
        if secret.public_key() != chain.genesis().authority() {
            return Err(StoreError::SchemaMismatch {
                detail: "Space authority does not match genesis",
            });
        }
        let ticket =
            SignedInviteTicket::sign(space_id, creator, owner_addr, validity, entropy, &secret)?;
        let committed = self.create_invitation_projected(
            &crate::InvitationRecord::new(
                ticket.invitation_id(),
                space_id,
                ticket.secret_digest(),
                ticket.creator(),
                i64::try_from(ticket.created_at_ms()).map_err(|_| StoreError::SchemaMismatch {
                    detail: "invitation creation exceeds SQLite range",
                })?,
                i64::try_from(ticket.expires_at_ms()).map_err(|_| StoreError::SchemaMismatch {
                    detail: "invitation expiry exceeds SQLite range",
                })?,
                ticket.encoded_owner_addr().to_vec(),
            ),
            |transaction| load_chain(transaction, space_id)?.ok_or(StoreError::SpaceNotFound),
        )?;
        Ok(CreatedEnrollmentInvite {
            revision: committed.revision(),
            ticket,
            chain: committed.into_value(),
        })
    }

    /// Atomically consumes an invitation and commits the candidate's authority generation.
    ///
    /// # Errors
    ///
    /// Returns an error when authority material, chain validation, or the transaction fails.
    #[allow(
        clippy::too_many_lines,
        reason = "redemption validation and mutation must remain one auditable atomic transaction"
    )]
    pub fn redeem_enrollment(
        &mut self,
        authorized: &AuthorizedEnrollmentRedemption,
    ) -> Result<EnrollmentOutcome, StoreError> {
        let ticket = &authorized.ticket;
        let input = &authorized.candidate;
        let now_ms = authorized.owner_now_ms;
        let row = self.connection.query_row(
            "SELECT space_id, authority_key_ref, creator_endpoint_id, created_at_ms, expires_at_ms, owner_bootstrap
             FROM invitations JOIN spaces USING(space_id)
             WHERE invitation_id = ?1 AND token_hash = ?2",
            (ticket.invitation_id().as_slice(), ticket.secret_digest().as_slice()),
            |row| Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Vec<u8>>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, Vec<u8>>(5)?,
            )),
        ).optional()?;
        let Some((
            space_bytes,
            reference,
            creator_bytes,
            created_at_ms,
            expires_at_ms,
            owner_bootstrap,
        )) = row
        else {
            return Ok(EnrollmentOutcome::NotFound);
        };
        let space_id = ma2a_core::SpaceId::try_from(space_bytes.as_slice()).map_err(|_| {
            StoreError::SchemaMismatch {
                detail: "invitation Space id is invalid",
            }
        })?;
        let creator = EndpointId::try_from(creator_bytes.as_slice()).map_err(|_| {
            StoreError::SchemaMismatch {
                detail: "invitation creator Endpoint id is invalid",
            }
        })?;
        if ticket.creator() != creator
            || i64::try_from(ticket.created_at_ms()).ok() != Some(created_at_ms)
            || i64::try_from(ticket.expires_at_ms()).ok() != Some(expires_at_ms)
            || ticket.encoded_owner_addr() != owner_bootstrap
        {
            return Ok(EnrollmentOutcome::NotFound);
        }
        let reference = reference
            .ok_or(StoreError::SpaceAuthorityUnavailable)
            .and_then(|value| KeyReference::parse(&value))?;
        let protected = self.key_store.read(KeyKind::SpaceAuthority, &reference)?;
        let secret = SpaceAuthoritySecret::try_from_bytes(protected.as_bytes())?;
        if ticket.verify(secret.public_key()).is_err() {
            return Ok(EnrollmentOutcome::NotFound);
        }
        if ticket.space_id() != space_id || ticket.creator() == input.endpoint_id {
            return Ok(EnrollmentOutcome::NotFound);
        }
        let transaction = self.immediate()?;
        let state = transaction.query_row(
            "SELECT status, expires_at_ms, consumed_by_endpoint_id, consumed_request_id, response_chain FROM invitations WHERE invitation_id = ?1 AND token_hash = ?2",
            (ticket.invitation_id().as_slice(), ticket.secret_digest().as_slice()),
            |row| Ok((row.get::<_, u8>(0)?, row.get::<_, i64>(1)?, row.get::<_, Option<Vec<u8>>>(2)?, row.get::<_, Option<Vec<u8>>>(3)?, row.get::<_, Option<Vec<u8>>>(4)?)),
        )?;
        match state.0 {
            1 => {
                if state.2.as_deref() == Some(input.endpoint_id.as_bytes())
                    && state.3.as_deref() == Some(input.request_id.as_bytes())
                {
                    return Ok(EnrollmentOutcome::Retry {
                        chain: state.4.ok_or(StoreError::SchemaMismatch {
                            detail: "consumed invitation response missing",
                        })?,
                    });
                }
                return Ok(EnrollmentOutcome::Conflict);
            }
            2 => return Ok(EnrollmentOutcome::Cancelled),
            3 => return Ok(EnrollmentOutcome::Expired),
            _ if now_ms < created_at_ms || state.1 <= now_ms => {
                return Ok(EnrollmentOutcome::Expired);
            }
            _ => {}
        }
        let mut chain = load_chain(&transaction, space_id)?.ok_or(StoreError::SpaceNotFound)?;
        if secret.public_key() != chain.genesis().authority() {
            return Err(StoreError::SchemaMismatch {
                detail: "Space authority does not match genesis",
            });
        }
        let (mut members, mut revocations) = chain.manifests().last().map_or_else(
            || {
                (
                    vec![chain.genesis().genesis().initial_member().clone()],
                    Vec::new(),
                )
            },
            |manifest| (manifest.members().to_vec(), manifest.revocations().to_vec()),
        );
        // A previously removed Endpoint rejoins only through a fresh owner-signed
        // invitation, and the new generation must carry membership rather than a
        // stale revocation for it. Earlier generations keep their revocation.
        revocations.retain(|revocation| revocation.endpoint_id() != input.endpoint_id);
        if members
            .binary_search_by_key(&input.endpoint_id, SpaceMemberV1::endpoint_id)
            .is_ok()
        {
            return Ok(EnrollmentOutcome::Conflict);
        }
        let candidate = SpaceMemberV1::new(
            input.endpoint_id,
            input.display_name.clone(),
            MemberCapabilities::new(true, false),
        )?;
        members.push(candidate);
        members.sort_by_key(SpaceMemberV1::endpoint_id);
        members.dedup_by_key(|member| member.endpoint_id());
        let generation =
            chain
                .latest_generation()
                .checked_add(1)
                .ok_or(StoreError::SchemaMismatch {
                    detail: "Space generation exhausted",
                })?;
        let manifest = SpaceManifestV1::new(
            SpaceManifestLink::new(space_id, generation, chain.latest_hash()),
            u64::try_from(now_ms).map_err(|_| StoreError::SchemaMismatch {
                detail: "negative enrollment timestamp",
            })?,
            SpaceManifestMembership::new(members, revocations),
        )?
        .sign(&secret)?;
        chain.apply(&manifest)?;
        let response = chain
            .export_public()
            .map_err(|_| StoreError::SchemaMismatch {
                detail: "enrollment chain export failed",
            })?;
        replace_chain(&transaction, &chain, Some(&reference))?;
        transaction.execute(
            "UPDATE invitations SET status = 1, consumed_at_ms = ?1, consumed_by_endpoint_id = ?2, consumed_request_id = ?3, response_chain = ?4 WHERE invitation_id = ?5 AND status = 0",
            (now_ms, input.endpoint_id.as_bytes().as_slice(), input.request_id.as_bytes().as_slice(), response.as_slice(), ticket.invitation_id().as_slice()),
        )?;
        let revision = increment_revision(&transaction)?;
        transaction.commit()?;
        Ok(EnrollmentOutcome::Redeemed {
            revision,
            chain: response,
        })
    }
}
