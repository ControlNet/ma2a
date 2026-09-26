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
    /// The exchange failed; the remote authority outcome may be unknown.
    pub const UNREACHABLE: Self = Self(DepartureErrorKind::Unreachable);
    /// The authority refused the request or returned an unusable chain.
    pub const REJECTED: Self = Self(DepartureErrorKind::Rejected);
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
    use super::{DEPARTURE_MAGIC, accepts_departure, decode_departure, encode_departure};
    use ma2a_core::{
        MemberCapabilities, ProtocolError, RequestId, SpaceAuthoritySecret, SpaceChain,
        SpaceGenesisIdentity, SpaceGenesisOwner, SpaceGenesisV1, SpaceId, SpaceManifestLink,
        SpaceManifestMembership, SpaceManifestV1, SpaceMemberV1, SpacePolicyV1, SpaceRevocationV1,
        default_member_label,
    };
    use ma2a_net::EndpointSecret;

    type TestValue<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
    type TestResult = TestValue<()>;

    fn endpoint(seed: u8) -> TestValue<ma2a_core::EndpointId> {
        Ok(EndpointSecret::parse(&[seed; 32])?.endpoint_id())
    }

    fn member(id: ma2a_core::EndpointId) -> Result<SpaceMemberV1, ProtocolError> {
        SpaceMemberV1::new(
            id,
            default_member_label(id),
            MemberCapabilities::new(true, false),
        )
    }

    /// Builds a chain whose generation 1 holds both members, and returns the
    /// authority so a caller can sign further generations.
    fn shared_space(nonce: u8) -> TestValue<(SpaceChain, SpaceAuthoritySecret)> {
        let secret = SpaceAuthoritySecret::from_bytes([nonce ^ 0x5a; 32]);
        let owner = endpoint(0x41)?;
        let genesis = SpaceGenesisV1::new(
            SpaceGenesisIdentity::new([nonce; 32], 1_700_000_000_000, secret.public_key())?,
            SpaceGenesisOwner::new(member(owner)?, SpacePolicyV1::phase_one_default()),
        )
        .with_name("lab")?
        .sign(&secret)?;
        let mut chain = SpaceChain::from_genesis(genesis)?;
        let mut members = vec![member(owner)?, member(endpoint(0x42)?)?];
        members.sort_by_key(SpaceMemberV1::endpoint_id);
        let manifest = SpaceManifestV1::new(
            SpaceManifestLink::new(chain.space_id(), 1, chain.latest_hash()),
            1_700_000_000_001,
            SpaceManifestMembership::new(members, Vec::new()),
        )?
        .sign(&secret)?;
        chain.apply(&manifest)?;
        Ok((chain, secret))
    }

    fn with_departure(
        chain: &SpaceChain,
        secret: &SpaceAuthoritySecret,
        departing: ma2a_core::EndpointId,
    ) -> TestValue<SpaceChain> {
        let mut advanced = chain.clone();
        let members = chain
            .members()
            .iter()
            .filter(|entry| entry.endpoint_id() != departing)
            .cloned()
            .collect::<Vec<_>>();
        let manifest = SpaceManifestV1::new(
            SpaceManifestLink::new(
                chain.space_id(),
                chain.latest_generation() + 1,
                chain.latest_hash(),
            ),
            1_700_000_000_002,
            SpaceManifestMembership::new(members, vec![SpaceRevocationV1::new(departing)]),
        )?
        .sign(secret)?;
        advanced.apply(&manifest)?;
        Ok(advanced)
    }

    #[test]
    fn a_departure_response_must_remove_and_revoke_exactly_the_requester() -> TestResult {
        // Given
        let (chain, secret) = shared_space(0x31)?;
        let departing = endpoint(0x42)?;

        // When
        let advanced = with_departure(&chain, &secret, departing)?;

        // Then
        assert!(accepts_departure(&chain, &advanced, departing));
        // The owner is still a member of the advanced chain, so a response that
        // removed somebody else cannot satisfy this Endpoint's departure.
        assert!(!accepts_departure(&chain, &advanced, endpoint(0x41)?));
        Ok(())
    }

    #[test]
    fn a_rolled_back_or_unchanged_departure_response_is_rejected() -> TestResult {
        // Given
        let (chain, secret) = shared_space(0x32)?;
        let departing = endpoint(0x42)?;
        let advanced = with_departure(&chain, &secret, departing)?;

        // When / Then: the pre-departure chain still holds the requester.
        assert!(!accepts_departure(&chain, &chain, departing));
        // A response that regresses to an earlier generation is rejected.
        assert!(!accepts_departure(&advanced, &chain, departing));
        Ok(())
    }

    #[test]
    fn a_forked_departure_response_from_another_space_is_rejected() -> TestResult {
        // Given
        let (chain, _secret) = shared_space(0x33)?;
        let (other, other_secret) = shared_space(0x34)?;
        let departing = endpoint(0x42)?;

        // When
        let forked = with_departure(&other, &other_secret, departing)?;

        // Then
        assert_ne!(chain.space_id(), forked.space_id());
        assert!(!accepts_departure(&chain, &forked, departing));
        Ok(())
    }

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
