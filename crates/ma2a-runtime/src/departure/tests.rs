//! Departure request encoding and authority-side acceptance.

use super::{DEPARTURE_MAGIC, accepts_departure, decode_departure, encode_departure};
use ma2a_core::{
    MemberCapabilities, ProtocolError, RequestId, SpaceAuthoritySecret, SpaceChain,
    SpaceGenesisIdentity, SpaceGenesisOwner, SpaceGenesisV1, SpaceId, SpaceManifestLink,
    SpaceManifestMembership, SpaceManifestV1, SpaceMemberV1, SpacePolicyV1, SpaceRevocationV1,
    default_member_label,
};
use ma2a_net::EndpointSecret;

pub(super) type TestValue<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
pub(super) type TestResult = TestValue<()>;

pub(super) fn endpoint(seed: u8) -> TestValue<ma2a_core::EndpointId> {
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
pub(super) fn shared_space(nonce: u8) -> TestValue<(SpaceChain, SpaceAuthoritySecret)> {
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

pub(super) fn with_departure(
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

/// Signs the next generation, which adds `joining` and keeps every revocation.
pub(super) fn with_member(
    chain: &SpaceChain,
    secret: &SpaceAuthoritySecret,
    joining: ma2a_core::EndpointId,
) -> TestValue<SpaceChain> {
    let mut advanced = chain.clone();
    let mut members = chain.members().to_vec();
    members.push(member(joining)?);
    members.sort_by_key(SpaceMemberV1::endpoint_id);
    let manifest = SpaceManifestV1::new(
        SpaceManifestLink::new(
            chain.space_id(),
            chain.latest_generation() + 1,
            chain.latest_hash(),
        ),
        1_700_000_000_003,
        SpaceManifestMembership::new(members, chain.revocations().to_vec()),
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
