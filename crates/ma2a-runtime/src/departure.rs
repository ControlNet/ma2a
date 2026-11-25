//! Member-initiated Space departure carried over the reserved enrollment ALPN.
//!
//! Leaving a Space is a membership change, not a local deletion. The leaving
//! member asks the Space authority to sign the next manifest generation without
//! it; only that authority-signed chain is persisted locally.

use std::{error::Error, fmt};

use ma2a_core::{EndpointId, RequestId, SpaceChain, SpaceId};

mod receipt;

pub(crate) use receipt::{through_generation, through_removal};

/// Magic prefix separating a departure request from an enrollment attempt on the
/// shared bootstrap ALPN.
pub(crate) const DEPARTURE_MAGIC: [u8; 4] = *b"MLV1";
const DEPARTURE_REQUEST_LEN: usize = DEPARTURE_MAGIC.len() + 32 + 16;

/// Status byte reported by the authority for one departure request.
pub(crate) const DEPARTURE_STATUS_OK: u8 = 0;
pub(crate) const DEPARTURE_STATUS_REJECTED: u8 = 1;
pub(crate) const DEPARTURE_STATUS_INTERNAL: u8 = 255;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DepartureErrorKind {
    NotAMember,
    OwnerCannotLeave,
    Unreachable,
    Rejected,
    InFlight,
    Busy,
    Internal,
}

/// Stable machine-readable classification of a Space departure failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpaceDepartureErrorCode(DepartureErrorKind);

impl SpaceDepartureErrorCode {
    /// The local Endpoint holds no current signed membership in that Space.
    pub const NOT_A_MEMBER: Self = Self(DepartureErrorKind::NotAMember);
    /// The local Endpoint owns the Space authority and cannot remove itself.
    pub const OWNER_CANNOT_LEAVE: Self = Self(DepartureErrorKind::OwnerCannotLeave);
    /// The exchange failed; the remote authority outcome may be unknown.
    pub const UNREACHABLE: Self = Self(DepartureErrorKind::Unreachable);
    /// The authority refused the request or returned an unusable chain.
    pub const REJECTED: Self = Self(DepartureErrorKind::Rejected);
    /// A departure of the same Space is still waiting on the authority; nothing was sent.
    pub const IN_FLIGHT: Self = Self(DepartureErrorKind::InFlight);
    /// Too many departures are already waiting on their Space authorities; nothing was sent, so
    /// the same departure can be retried once one of them ends.
    pub const BUSY: Self = Self(DepartureErrorKind::Busy);
    /// The local Runtime failed before or after the exchange.
    pub const INTERNAL: Self = Self(DepartureErrorKind::Internal);
}

/// A typed failure returned by the Space departure API.
#[derive(Debug)]
pub struct SpaceDepartureError(DepartureErrorKind, Option<u64>);

impl SpaceDepartureError {
    pub(crate) const fn after_commit(mut self, revision: u64) -> Self {
        self.1 = Some(revision);
        self
    }

    /// Returns the local durable revision when departure committed before completion failed.
    pub const fn committed_revision(&self) -> Option<u64> {
        self.1
    }

    /// Returns the stable classification for this failure.
    pub const fn code(&self) -> SpaceDepartureErrorCode {
        SpaceDepartureErrorCode(self.0)
    }
    pub(crate) const fn not_a_member() -> Self {
        Self(DepartureErrorKind::NotAMember, None)
    }
    pub(crate) const fn owner_cannot_leave() -> Self {
        Self(DepartureErrorKind::OwnerCannotLeave, None)
    }
    pub(crate) const fn unreachable() -> Self {
        Self(DepartureErrorKind::Unreachable, None)
    }
    pub(crate) const fn rejected() -> Self {
        Self(DepartureErrorKind::Rejected, None)
    }
    pub(crate) const fn in_flight() -> Self {
        Self(DepartureErrorKind::InFlight, None)
    }
    pub(crate) const fn busy() -> Self {
        Self(DepartureErrorKind::Busy, None)
    }
    pub(crate) const fn internal() -> Self {
        Self(DepartureErrorKind::Internal, None)
    }
}

impl fmt::Display for SpaceDepartureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(revision) = self.1 {
            return write!(
                formatter,
                "Space departure committed at revision {revision}; completion remains pending"
            );
        }
        formatter.write_str(match self.0 {
            DepartureErrorKind::NotAMember => "this Endpoint is not a member of that Space",
            DepartureErrorKind::OwnerCannotLeave => "Space owner cannot leave its own Space",
            DepartureErrorKind::Unreachable => {
                "departure exchange did not complete; authority outcome is unknown"
            }
            DepartureErrorKind::Rejected => "the Space authority rejected the departure request",
            DepartureErrorKind::InFlight => "a departure of this Space is already in progress",
            DepartureErrorKind::Busy => {
                "too many Space departures are pending; retry once one ends"
            }
            DepartureErrorKind::Internal => "Space departure failed",
        })
    }
}

impl Error for SpaceDepartureError {}

pub(crate) fn encode_departure(space_id: SpaceId, request_id: RequestId) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(DEPARTURE_REQUEST_LEN);
    bytes.extend_from_slice(&DEPARTURE_MAGIC);
    bytes.extend_from_slice(space_id.as_bytes());
    bytes.extend_from_slice(request_id.as_bytes());
    bytes
}

pub(crate) fn decode_departure(bytes: &[u8]) -> Option<(SpaceId, RequestId)> {
    if bytes.len() != DEPARTURE_REQUEST_LEN || !bytes.starts_with(&DEPARTURE_MAGIC) {
        return None;
    }
    let space_id = SpaceId::try_from(bytes.get(4..36)?).ok()?;
    let request_id = RequestId::try_from(bytes.get(36..52)?).ok()?;
    Some((space_id, request_id))
}

/// Whether `local` may answer `departing`'s request to leave `chain`: only the Space authority
/// signs a removal, and Phase 1 has no authority transfer, so that is the genesis owner; the
/// owner itself never leaves.
pub(crate) fn answers_departure(
    chain: &SpaceChain,
    local: EndpointId,
    departing: EndpointId,
) -> bool {
    let owner = chain.genesis().genesis().initial_member().endpoint_id();
    owner == local && owner != departing
}

/// Confirms that `chain` is the same Space, advanced by its own authority, and
/// that it removes and revokes exactly the departing Endpoint.
pub(crate) fn accepts_departure(
    previous: &SpaceChain,
    advanced: &SpaceChain,
    departing: EndpointId,
) -> bool {
    advanced.space_id() == previous.space_id()
        && advanced.genesis() == previous.genesis()
        && advanced.latest_generation() >= previous.latest_generation()
        && !advanced
            .members()
            .iter()
            .any(|member| member.endpoint_id() == departing)
        && advanced
            .revocations()
            .iter()
            .any(|revocation| revocation.endpoint_id() == departing)
}

#[cfg(test)]
mod tests;
