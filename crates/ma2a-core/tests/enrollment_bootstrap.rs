//! Enrollment bootstrap envelope protocol coverage.

use iroh_base::{SecretKey, TransportAddr};
use ma2a_core::{
    AddressEndpointDataV1, AddressRecordScope, AddressRecordValidity, EnrollmentBootstrap,
    MAX_ENROLLMENT_BOOTSTRAP_FRAME_BYTES, MemberCapabilities, SpaceAddressRecordV1,
    SpaceAuthoritySecret, SpaceChain, SpaceGenesisIdentity, SpaceGenesisOwner, SpaceGenesisV1,
    SpaceMemberV1, SpacePolicyV1,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn bootstrap_frames_roundtrip_exact_chain_and_owner_record() -> TestResult {
    // Given
    let (chain, owner_record) = fixture()?;
    let bootstrap = EnrollmentBootstrap::new(chain.clone(), owner_record.clone())?;

    // When
    let frames = bootstrap.encode_frames()?;
    let decoded = EnrollmentBootstrap::decode_frames(&frames)?;

    // Then
    assert_eq!(decoded.chain(), &chain);
    assert_eq!(decoded.owner_address_record(), &owner_record);
    assert_eq!(decoded.encode_frames()?, frames);
    assert!(
        frames
            .iter()
            .all(|frame| frame.len() <= MAX_ENROLLMENT_BOOTSTRAP_FRAME_BYTES)
    );
    Ok(())
}

#[test]
fn bootstrap_rejects_missing_owner_record_and_trailing_bytes() -> TestResult {
    // Given
    let (chain, owner_record) = fixture()?;
    let frames =
        EnrollmentBootstrap::new(chain, owner_record).and_then(|value| value.encode_frames())?;
    let mut missing_owner = frames.clone();
    let first = missing_owner.first_mut().ok_or("bootstrap frame missing")?;
    let page_length = usize::try_from(u32::from_be_bytes(
        first.get(4..8).ok_or("page length missing")?.try_into()?,
    ))?;
    first.truncate(8 + page_length + 4);
    first
        .get_mut(8 + page_length..)
        .ok_or("owner address length missing")?
        .copy_from_slice(&0_u32.to_be_bytes());
    let mut trailing = frames;
    trailing
        .first_mut()
        .ok_or("bootstrap frame missing")?
        .push(0);

    // When
    let missing_result = EnrollmentBootstrap::decode_frames(&missing_owner);
    let trailing_result = EnrollmentBootstrap::decode_frames(&trailing);

    // Then
    assert!(missing_result.is_err());
    assert!(trailing_result.is_err());
    Ok(())
}

fn fixture()
-> Result<(SpaceChain, ma2a_core::SignedSpaceAddressRecordV1), Box<dyn std::error::Error>> {
    let authority = SpaceAuthoritySecret::from_bytes([0x31; 32]);
    let owner = SecretKey::from_bytes(&[0x32; 32]);
    let member = SpaceMemberV1::new(
        owner.public().into(),
        "owner".to_owned(),
        MemberCapabilities::new(true, false),
    )?;
    let genesis = SpaceGenesisV1::new(
        SpaceGenesisIdentity::new([0x33; 32], 1_000, authority.public_key())?,
        SpaceGenesisOwner::new(member, SpacePolicyV1::phase_one_default()),
    )
    .sign(&authority)?;
    let chain = SpaceChain::from_genesis(genesis)?;
    let record = SpaceAddressRecordV1::new(
        AddressRecordScope::new(chain.space_id(), owner.public().into()),
        AddressRecordValidity::new(1, 1_000, 301_000)?,
        AddressEndpointDataV1::new(vec![TransportAddr::Ip("127.0.0.1:4242".parse()?)])?,
    )
    .sign(&owner)?;
    Ok((chain, record))
}
