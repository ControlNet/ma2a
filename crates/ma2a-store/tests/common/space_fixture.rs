use crate::support::TestResultValue;

pub(crate) fn signed_space_genesis() -> TestResultValue<ma2a_core::SignedSpaceGenesisV1> {
    let secret = ma2a_core::SpaceAuthoritySecret::from_bytes([0x41; 32]);
    let endpoint_secret = ma2a_core::SpaceAuthoritySecret::from_bytes([0x43; 32]);
    let endpoint =
        ma2a_core::EndpointId::try_from(endpoint_secret.public_key().as_bytes().as_slice())?;
    let member = ma2a_core::SpaceMemberV1::new(
        endpoint,
        "store-fixture-owner".to_owned(),
        ma2a_core::MemberCapabilities::new(true, true),
    )?;
    Ok(ma2a_core::SpaceGenesisV1::new(
        ma2a_core::SpaceGenesisIdentity::new([0x42; 32], 1, secret.public_key())?,
        ma2a_core::SpaceGenesisOwner::new(member, ma2a_core::SpacePolicyV1::phase_one_default()),
    )
    .sign(&secret)?)
}

pub(crate) fn space_record() -> TestResultValue<ma2a_store::SpaceRecord> {
    let genesis = signed_space_genesis()?;
    Ok(ma2a_store::SpaceRecord::new(
        genesis.space_id(),
        genesis.canonical_bytes().to_vec(),
    ))
}
