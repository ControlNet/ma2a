use ma2a_core::{
    EndpointId, MemberCapabilities, ProtocolError, SignedSpaceGenesisV1, SignedSpaceManifestV1,
    SpaceAuthoritySecret, SpaceGenesisIdentity, SpaceGenesisOwner, SpaceGenesisV1,
    SpaceManifestLink, SpaceManifestMembership, SpaceManifestV1, SpaceMemberV1, SpacePolicyV1,
};

pub(crate) const AUTHORITY_SECRET: [u8; 32] = [0x11; 32];
pub(crate) const GENESIS_NONCE: [u8; 32] = [0x33; 32];

pub(crate) fn endpoint(marker: u8) -> Result<EndpointId, ProtocolError> {
    const FIRST: [u8; 32] = [
        0xdb, 0x99, 0x5f, 0xe2, 0x51, 0x69, 0xd1, 0x41, 0xca, 0xb9, 0xbb, 0xba, 0x92, 0xba, 0xa0,
        0x1f, 0x9f, 0x2e, 0x1e, 0xce, 0x7d, 0xf4, 0xcb, 0x2a, 0xc0, 0x51, 0x90, 0xf3, 0x7f, 0xcc,
        0x1f, 0x9d,
    ];
    const SECOND: [u8; 32] = [
        0x21, 0x52, 0xf8, 0xd1, 0x9b, 0x79, 0x1d, 0x24, 0x45, 0x32, 0x42, 0xe1, 0x5f, 0x2e, 0xab,
        0x6c, 0xb7, 0xcf, 0xfa, 0x7b, 0x6a, 0x5e, 0xd3, 0x00, 0x97, 0x96, 0x0e, 0x06, 0x98, 0x81,
        0xdb, 0x12,
    ];
    let bytes = match marker {
        0x66 => FIRST,
        0x77 => SECOND,
        _ => return Err(ProtocolError::INVALID_INPUT),
    };
    EndpointId::try_from(bytes.as_slice())
}

pub(crate) fn member(marker: u8, relay: bool) -> Result<SpaceMemberV1, ProtocolError> {
    SpaceMemberV1::new(
        endpoint(marker)?,
        format!("endpoint-{marker:02x}"),
        MemberCapabilities::new(true, relay),
    )
}

pub(crate) fn signed_genesis() -> Result<SignedSpaceGenesisV1, ProtocolError> {
    let secret = SpaceAuthoritySecret::from_bytes(AUTHORITY_SECRET);
    SpaceGenesisV1::new(
        SpaceGenesisIdentity::new(GENESIS_NONCE, 1_700_000_000_000, secret.public_key())?,
        SpaceGenesisOwner::new(member(0x66, true)?, SpacePolicyV1::phase_one_default()),
    )
    .sign(&secret)
}

pub(crate) fn signed_manifest(
    genesis: &SignedSpaceGenesisV1,
) -> Result<SignedSpaceManifestV1, ProtocolError> {
    let secret = SpaceAuthoritySecret::from_bytes(AUTHORITY_SECRET);
    SpaceManifestV1::new(
        SpaceManifestLink::new(genesis.space_id(), 1, genesis.chain_hash()),
        1_700_000_001_000,
        SpaceManifestMembership::new(vec![member(0x77, false)?, member(0x66, true)?], Vec::new()),
    )?
    .sign(&secret)
}
