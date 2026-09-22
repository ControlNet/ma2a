//! Member-initiated Space departure carried over the reserved enrollment ALPN.
//!
//! Leaving a Space is a membership change, not a local deletion. The leaving
//! member asks the Space authority to sign the next manifest generation without
//! it; only that authority-signed chain is persisted locally.

use std::{error::Error, fmt};

use ma2a_core::{EndpointId, RequestId, SpaceChain, SpaceId};

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
    /// The Space authority could not be reached, so nothing was removed.
    pub const UNREACHABLE: Self = Self(DepartureErrorKind::Unreachable);
    /// The authority refused the request or returned an unusable chain.
    pub const REJECTED: Self = Self(DepartureErrorKind::Rejected);
    /// The local Runtime failed before or after the exchange.
    pub const INTERNAL: Self = Self(DepartureErrorKind::Internal);
}

/// A typed failure returned by the Space departure API.
#[derive(Debug)]
pub struct SpaceDepartureError(DepartureErrorKind);

impl SpaceDepartureError {
    /// Returns the stable classification for this failure.
    pub const fn code(&self) -> SpaceDepartureErrorCode {
        SpaceDepartureErrorCode(self.0)
    }
    pub(crate) const fn not_a_member() -> Self {
        Self(DepartureErrorKind::NotAMember)
    }
    pub(crate) const fn owner_cannot_leave() -> Self {
        Self(DepartureErrorKind::OwnerCannotLeave)
    }
    pub(crate) const fn unreachable() -> Self {
        Self(DepartureErrorKind::Unreachable)
    }
    pub(crate) const fn rejected() -> Self {
        Self(DepartureErrorKind::Rejected)
    }
    pub(crate) const fn internal() -> Self {
        Self(DepartureErrorKind::Internal)
    }
}

impl fmt::Display for SpaceDepartureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.0 {
            DepartureErrorKind::NotAMember => "this Endpoint is not a member of that Space",
            DepartureErrorKind::OwnerCannotLeave => "Space owner cannot leave its own Space",
            DepartureErrorKind::Unreachable => {
                "the Space authority could not be reached; membership is unchanged"
            }
            DepartureErrorKind::Rejected => "the Space authority rejected the departure request",
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
mod tests {
    use super::{DEPARTURE_MAGIC, decode_departure, encode_departure};
    use ma2a_core::{RequestId, SpaceId};

    #[test]
    fn departure_requests_round_trip_and_reject_foreign_frames() {
        // Given
        let space_id = SpaceId::try_from([0x21_u8; 32].as_slice()).expect("Space identifier");
        let request_id = RequestId::try_from([0x37_u8; 16].as_slice()).expect("request identifier");

        // When
        let encoded = encode_departure(space_id, request_id);

        // Then
        assert_eq!(decode_departure(&encoded), Some((space_id, request_id)));
        assert_eq!(
            decode_departure(encoded.get(..51).unwrap_or_default()),
            None
        );
        assert_eq!(decode_departure(b"not-a-departure-frame"), None);
        assert!(encoded.starts_with(&DEPARTURE_MAGIC));
    }
}
