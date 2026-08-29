use ma2a_core::{EndpointId, RequestId, SignedInviteTicket};

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
    pub(crate) ticket: SignedInviteTicket,
    pub(crate) candidate: EnrollmentRedemption,
    pub(crate) owner_now_ms: i64,
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
