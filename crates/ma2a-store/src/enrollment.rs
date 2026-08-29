use iroh_base::EndpointAddr;
use ma2a_core::{
    EndpointId, InviteEntropy, InviteValidity, MemberCapabilities, RequestId, SignedInviteTicket,
    SpaceAuthoritySecret, SpaceId, SpaceManifestLink, SpaceManifestMembership, SpaceManifestV1,
    SpaceMemberV1,
};
use rusqlite::OptionalExtension as _;

use crate::{
    KeyKind, KeyReference, Repository, StoreError,
    repository::increment_revision,
    space_rows::{load_chain, replace_chain},
};

#[derive(Clone, Debug, PartialEq, Eq)]
/// Authenticated candidate data used for atomic invitation redemption.
#[allow(
    clippy::exhaustive_structs,
    reason = "runtime and store exchange this closed internal protocol record"
)]
pub struct EnrollmentRedemption {
    /// Candidate identity authenticated by the transport.
    pub endpoint_id: EndpointId,
    /// Stable request identifier used for exact retry detection.
    pub request_id: RequestId,
    /// Candidate display name added to the next manifest generation.
    pub display_name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// Verified ticket, authenticated candidate, and owner-authoritative redemption time.
pub struct AuthorizedEnrollmentRedemption {
    ticket: SignedInviteTicket,
    candidate: EnrollmentRedemption,
    owner_now_ms: i64,
}

impl AuthorizedEnrollmentRedemption {
    /// Binds candidate data to a verified ticket and owner clock value.
    pub const fn new(
        ticket: SignedInviteTicket,
        candidate: EnrollmentRedemption,
        owner_now_ms: i64,
    ) -> Self {
        Self {
            ticket,
            candidate,
            owner_now_ms,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// Durable result of attempting to redeem an enrollment invitation.
#[allow(
    clippy::exhaustive_enums,
    reason = "runtime exhaustively maps the closed store outcome protocol"
)]
pub enum EnrollmentOutcome {
    /// The invitation was consumed and a new authority chain was committed.
    Redeemed {
        /// Repository revision after the atomic commit.
        revision: u64,
        /// Canonical complete Space chain returned to the candidate.
        chain: Vec<u8>,
    },
    /// The same candidate and request repeated a previously committed redemption.
    Retry {
        /// Original canonical complete Space chain stored with the commit.
        chain: Vec<u8>,
    },
    /// No valid invitation matched the supplied identifier and digest.
    NotFound,
    /// The invitation expired before redemption.
    Expired,
    /// The owner cancelled the invitation before redemption.
    Cancelled,
    /// The invitation was already consumed by a different candidate or request.
    Conflict,
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
    ) -> Result<SignedInviteTicket, StoreError> {
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
        self.create_invitation(&crate::InvitationRecord::new(
            ticket.invitation_id(),
            space_id,
            ticket.secret_digest(),
            i64::try_from(ticket.expires_at_ms()).map_err(|_| StoreError::SchemaMismatch {
                detail: "invitation expiry exceeds SQLite range",
            })?,
        ))?;
        Ok(ticket)
    }

    /// Cancels a pending invitation and advances the repository revision.
    ///
    /// # Errors
    ///
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
            "SELECT space_id, authority_key_ref FROM invitations JOIN spaces USING(space_id) WHERE invitation_id = ?1 AND token_hash = ?2",
            (ticket.invitation_id().as_slice(), ticket.secret_digest().as_slice()),
            |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Option<String>>(1)?)),
        ).optional()?;
        let Some((space_bytes, reference)) = row else {
            return Ok(EnrollmentOutcome::NotFound);
        };
        let space_id = ma2a_core::SpaceId::try_from(space_bytes.as_slice()).map_err(|_| {
            StoreError::SchemaMismatch {
                detail: "invitation Space id is invalid",
            }
        })?;
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
            _ if state.1 <= now_ms => return Ok(EnrollmentOutcome::Expired),
            _ => {}
        }
        let mut chain = load_chain(&transaction, space_id)?.ok_or(StoreError::SpaceNotFound)?;
        if secret.public_key() != chain.genesis().authority() {
            return Err(StoreError::SchemaMismatch {
                detail: "Space authority does not match genesis",
            });
        }
        let mut members = chain.manifests().last().map_or_else(
            || vec![chain.genesis().genesis().initial_member().clone()],
            |manifest| manifest.members().to_vec(),
        );
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
            SpaceManifestMembership::new(members, vec![]),
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
