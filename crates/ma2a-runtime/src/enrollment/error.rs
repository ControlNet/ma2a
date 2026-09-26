use std::{error::Error, fmt};

/// Enrollment stages whose failures can be diagnosed without exposing invitation material.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum EnrollmentStage {
    /// Dial, ALPN, or framed transfer; the network error identifies its substage.
    Exchange,
    /// Owner invite validation or redemption transaction.
    OwnerRedemption,
    /// Bootstrap decoding and chain validation.
    BootstrapValidation,
    /// Candidate membership and owner-address persistence.
    CandidatePersistence,
    /// Private lookup refresh from durable Store truth.
    ControlLookup,
    /// Relay candidate projection application.
    RelayCandidates,
    /// Signed address and relay publication.
    Publications,
    /// Initial targeted control scheduling.
    TargetedControl,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EnrollmentErrorKind {
    InvalidTicket,
    Expired,
    Cancelled,
    Conflict,
    Internal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Stable machine-readable classification of an enrollment failure.
pub struct EnrollmentErrorCode(EnrollmentErrorKind);

impl EnrollmentErrorCode {
    /// The invitation is unknown, malformed, or invalid for this candidate.
    pub const INVALID_TICKET: Self = Self(EnrollmentErrorKind::InvalidTicket);
    /// The invitation validity window has ended.
    pub const EXPIRED: Self = Self(EnrollmentErrorKind::Expired);
    /// The owner cancelled the invitation before redemption.
    pub const CANCELLED: Self = Self(EnrollmentErrorKind::Cancelled);
    /// Another candidate or request already consumed the invitation.
    pub const CONFLICT: Self = Self(EnrollmentErrorKind::Conflict);
}

#[derive(Debug)]
/// A typed failure returned by the enrollment API.
pub struct EnrollmentError(EnrollmentErrorKind, Option<EnrollmentStage>, Option<u64>);

impl EnrollmentError {
    /// Returns the stable classification for this failure.
    pub const fn code(&self) -> EnrollmentErrorCode {
        EnrollmentErrorCode(self.0)
    }
    pub(crate) const fn internal() -> Self {
        Self(EnrollmentErrorKind::Internal, None, None)
    }
    pub(crate) const fn from_status(status: u8) -> Self {
        Self(
            match status {
                1 => EnrollmentErrorKind::InvalidTicket,
                2 => EnrollmentErrorKind::Expired,
                3 => EnrollmentErrorKind::Cancelled,
                4 => EnrollmentErrorKind::Conflict,
                _ => EnrollmentErrorKind::Internal,
            },
            None,
            None,
        )
    }

    pub(crate) fn at_stage(mut self, stage: EnrollmentStage) -> Self {
        self.1 = Some(stage);
        eprintln!(
            "enrollment failed at {stage:?}; committed_revision={:?}",
            self.2
        );
        self
    }

    pub(crate) const fn after_commit(mut self, revision: u64) -> Self {
        self.2 = Some(revision);
        self
    }

    /// Returns the local membership commit revision when completion failed afterwards.
    pub const fn committed_revision(&self) -> Option<u64> {
        self.2
    }

    /// Returns the diagnosed stage without ticket or protected material.
    pub const fn stage(&self) -> Option<EnrollmentStage> {
        self.1
    }
}

impl fmt::Display for EnrollmentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(revision) = self.2 {
            return write!(
                formatter,
                "enrollment membership committed at revision {revision}; completion pending at {:?}",
                self.1
            );
        }
        formatter.write_str(match self.0 {
            EnrollmentErrorKind::InvalidTicket => "enrollment invitation is invalid",
            EnrollmentErrorKind::Expired => "enrollment invitation expired",
            EnrollmentErrorKind::Cancelled => "enrollment invitation was cancelled",
            EnrollmentErrorKind::Conflict => {
                "enrollment invitation was consumed by another candidate"
            }
            EnrollmentErrorKind::Internal => "enrollment exchange failed",
        })
    }
}

impl Error for EnrollmentError {}
