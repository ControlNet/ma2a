use iroh_tickets::Ticket as _;
use ma2a_core::{
    EndpointId, InviteEntropy, InviteValidity, MAX_MEMBER_LABEL_LEN, RequestId, SignedInviteTicket,
    SpaceId,
};
use ma2a_store::EnrollmentRedemption;

pub(crate) mod completion;
mod error;
mod owner;
pub use error::{EnrollmentError, EnrollmentErrorCode, EnrollmentStage};

#[cfg(test)]
#[path = "enrollment/tests.rs"]
mod tests;

#[derive(Clone, Debug, PartialEq, Eq)]
/// Parameters for issuing a single-use enrollment invitation.
pub struct EnrollmentCreation {
    space_id: SpaceId,
    lifetime_ms: u64,
    entropy: InviteEntropy,
}

impl EnrollmentCreation {
    /// Creates invitation parameters for a Space and bounded lifetime.
    ///
    /// # Errors
    /// Returns an error unless the invitation lifetime is positive and at most five minutes.
    pub fn new(
        space_id: SpaceId,
        lifetime_ms: u64,
        entropy: InviteEntropy,
    ) -> Result<Self, ma2a_core::ProtocolError> {
        let _validity = InviteValidity::for_lifetime(0, lifetime_ms)?;
        Ok(Self {
            space_id,
            lifetime_ms,
            entropy,
        })
    }

    /// Returns the Space to which the invitation grants membership.
    pub const fn space_id(&self) -> SpaceId {
        self.space_id
    }
    pub(crate) fn issue_at(
        self,
        created_at_ms: u64,
    ) -> Result<IssuedEnrollmentCreation, ma2a_core::ProtocolError> {
        Ok(IssuedEnrollmentCreation {
            space_id: self.space_id,
            validity: InviteValidity::for_lifetime(created_at_ms, self.lifetime_ms)?,
            entropy: self.entropy,
        })
    }
}

pub(crate) struct IssuedEnrollmentCreation {
    pub(crate) space_id: SpaceId,
    pub(crate) validity: InviteValidity,
    pub(crate) entropy: InviteEntropy,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// A candidate's request to redeem a signed invitation.
pub struct EnrollmentAttempt {
    ticket: SignedInviteTicket,
    request_id: RequestId,
    display_name: String,
}

impl EnrollmentAttempt {
    /// Creates an enrollment attempt with a stable idempotency key.
    #[allow(
        clippy::too_many_arguments,
        reason = "the request boundary keeps ticket, idempotency, identity, and time explicit"
    )]
    pub const fn new(
        ticket: SignedInviteTicket,
        request_id: RequestId,
        display_name: String,
    ) -> Self {
        Self {
            ticket,
            request_id,
            display_name,
        }
    }

    pub(crate) fn owner_addr(&self) -> ma2a_net::EndpointAddr {
        self.ticket.owner_addr()
    }
    pub(crate) const fn space_id(&self) -> SpaceId {
        self.ticket.space_id()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// Confirmation that the candidate durably established Space membership.
pub struct EstablishedEnrollment {
    committed: ma2a_store::Committed<ma2a_core::SpaceChain>,
}

impl EstablishedEnrollment {
    /// Returns the latest persisted authority generation.
    pub fn generation(&self) -> u64 {
        self.committed.value().latest_generation()
    }
    /// Returns the revision of the membership transaction.
    pub const fn revision(&self) -> u64 {
        self.committed.revision()
    }
    /// Returns the actual chain read inside the membership transaction.
    pub const fn chain(&self) -> &ma2a_core::SpaceChain {
        self.committed.value()
    }
    pub(crate) const fn new(revision: u64, chain: ma2a_core::SpaceChain) -> Self {
        Self {
            committed: ma2a_store::Committed::new(revision, chain),
        }
    }
}

pub(crate) fn encode_attempt(attempt: &EnrollmentAttempt) -> Result<Vec<u8>, EnrollmentError> {
    let ticket = attempt.ticket.encode_bytes();
    let ticket_len = u16::try_from(ticket.len()).map_err(|_| EnrollmentError::internal())?;
    let name = attempt.display_name.as_bytes();
    validate_display_name(&attempt.display_name)?;
    let name_len = u16::try_from(name.len()).map_err(|_| EnrollmentError::internal())?;
    let mut bytes = Vec::with_capacity(2 + ticket.len() + 16 + 2 + name.len());
    bytes.extend_from_slice(&ticket_len.to_be_bytes());
    bytes.extend_from_slice(&ticket);
    bytes.extend_from_slice(attempt.request_id.as_bytes());
    bytes.extend_from_slice(&name_len.to_be_bytes());
    bytes.extend_from_slice(name);
    Ok(bytes)
}

pub(crate) fn decode_attempt(
    bytes: &[u8],
    endpoint_id: EndpointId,
) -> Result<(SignedInviteTicket, EnrollmentRedemption), EnrollmentError> {
    let mut cursor = 0;
    let ticket_len = usize::from(u16::from_be_bytes(take::<2>(bytes, &mut cursor)?));
    let ticket_end = cursor
        .checked_add(ticket_len)
        .ok_or_else(EnrollmentError::internal)?;
    let ticket = SignedInviteTicket::decode_bytes(
        bytes
            .get(cursor..ticket_end)
            .ok_or_else(EnrollmentError::internal)?,
    )
    .map_err(|_| EnrollmentError::from_status(1))?;
    cursor = ticket_end;
    let request_id = RequestId::try_from(take::<16>(bytes, &mut cursor)?.as_slice())
        .map_err(|_| EnrollmentError::internal())?;
    let name_len = usize::from(u16::from_be_bytes(take::<2>(bytes, &mut cursor)?));
    let name_end = cursor
        .checked_add(name_len)
        .ok_or_else(EnrollmentError::internal)?;
    if name_end != bytes.len() {
        return Err(EnrollmentError::internal());
    }
    let display_name = std::str::from_utf8(
        bytes
            .get(cursor..name_end)
            .ok_or_else(EnrollmentError::internal)?,
    )
    .map_err(|_| EnrollmentError::internal())?
    .to_owned();
    validate_display_name(&display_name)?;
    let redemption = EnrollmentRedemption {
        endpoint_id,
        request_id,
        display_name,
    };
    Ok((ticket, redemption))
}

fn validate_display_name(display_name: &str) -> Result<(), EnrollmentError> {
    if display_name.is_empty()
        || display_name.len() > MAX_MEMBER_LABEL_LEN
        || display_name.chars().any(char::is_control)
    {
        return Err(EnrollmentError::internal());
    }
    Ok(())
}

fn take<const N: usize>(bytes: &[u8], cursor: &mut usize) -> Result<[u8; N], EnrollmentError> {
    let end = cursor
        .checked_add(N)
        .ok_or_else(EnrollmentError::internal)?;
    let value = <[u8; N]>::try_from(
        bytes
            .get(*cursor..end)
            .ok_or_else(EnrollmentError::internal)?,
    )
    .map_err(|_| EnrollmentError::internal())?;
    *cursor = end;
    Ok(value)
}
