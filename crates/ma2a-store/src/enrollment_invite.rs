use iroh_base::EndpointAddr;
use ma2a_core::{
    EndpointId, InviteEntropy, InviteValidity, SignedInviteTicket, SpaceAuthoritySecret, SpaceId,
};

use crate::{KeyKind, KeyReference, Repository, StoreError, space_rows::load_chain};

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
}
