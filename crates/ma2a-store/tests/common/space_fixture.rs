use crate::support::TestResultValue;

pub(crate) fn signed_space_genesis() -> TestResultValue<ma2a_core::SignedSpaceGenesisV1> {
    let secret = ma2a_core::SpaceAuthoritySecret::from_bytes([0x41; 32]);
    let endpoint = ma2a_core::EndpointId::try_from(
        [
            0xdb, 0x99, 0x5f, 0xe2, 0x51, 0x69, 0xd1, 0x41, 0xca, 0xb9, 0xbb, 0xba, 0x92, 0xba,
            0xa0, 0x1f, 0x9f, 0x2e, 0x1e, 0xce, 0x7d, 0xf4, 0xcb, 0x2a, 0xc0, 0x51, 0x90, 0xf3,
            0x7f, 0xcc, 0x1f, 0x9d,
        ]
        .as_slice(),
    )?;
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
