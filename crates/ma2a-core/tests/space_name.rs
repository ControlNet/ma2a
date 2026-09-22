//! The shared Space name is signed genesis metadata, not a local alias.

use ma2a_core::{
    EndpointId, MAX_SPACE_NAME_LEN, MemberCapabilities, ProtocolError, SignedSpaceGenesisV1,
    SpaceAuthoritySecret, SpaceChain, SpaceGenesisIdentity, SpaceGenesisOwner, SpaceGenesisV1,
    SpaceMemberV1, SpacePolicyV1,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const AUTHORITY_SECRET: [u8; 32] = [0x11; 32];
const GENESIS_NONCE: [u8; 32] = [0x33; 32];
const OWNER: [u8; 32] = [
    0xdb, 0x99, 0x5f, 0xe2, 0x51, 0x69, 0xd1, 0x41, 0xca, 0xb9, 0xbb, 0xba, 0x92, 0xba, 0xa0, 0x1f,
    0x9f, 0x2e, 0x1e, 0xce, 0x7d, 0xf4, 0xcb, 0x2a, 0xc0, 0x51, 0x90, 0xf3, 0x7f, 0xcc, 0x1f, 0x9d,
];

fn owner() -> Result<SpaceMemberV1, ProtocolError> {
    SpaceMemberV1::new(
        EndpointId::try_from(OWNER.as_slice())?,
        "endpoint-66".to_owned(),
        MemberCapabilities::new(true, true),
    )
}

fn unnamed() -> Result<SignedSpaceGenesisV1, ProtocolError> {
    let secret = SpaceAuthoritySecret::from_bytes(AUTHORITY_SECRET);
    SpaceGenesisV1::new(
        SpaceGenesisIdentity::new(GENESIS_NONCE, 1_700_000_000_000, secret.public_key())?,
        SpaceGenesisOwner::new(owner()?, SpacePolicyV1::phase_one_default()),
    )
    .sign(&secret)
}

fn named(nonce: [u8; 32], name: &str) -> Result<SignedSpaceGenesisV1, ProtocolError> {
    let secret = SpaceAuthoritySecret::from_bytes(AUTHORITY_SECRET);
    SpaceGenesisV1::new(
        SpaceGenesisIdentity::new(nonce, 1_700_000_000_000, secret.public_key())?,
        SpaceGenesisOwner::new(owner()?, SpacePolicyV1::phase_one_default()),
    )
    .with_name(name)?
    .sign(&secret)
}

#[test]
fn a_signed_space_name_survives_canonical_round_trips_and_chain_export() -> TestResult {
    // Given
    let genesis = named(GENESIS_NONCE, "lab")?;

    // When
    let decoded = SignedSpaceGenesisV1::from_canonical_bytes(genesis.canonical_bytes())?;
    let exported = SpaceChain::from_genesis(genesis.clone())?.export_public()?;
    let imported = SpaceChain::import_public(&exported)?;

    // Then
    assert_eq!(decoded, genesis);
    assert_eq!(decoded.name(), Some("lab"));
    assert_eq!(imported.genesis().name(), Some("lab"));
    assert_eq!(imported.space_id(), genesis.space_id());
    Ok(())
}

#[test]
fn the_name_is_bound_into_the_space_identifier() -> TestResult {
    // Given
    let unnamed = unnamed()?;

    // When
    let lab = named(GENESIS_NONCE, "lab")?;
    let ops = named(GENESIS_NONCE, "ops")?;

    // Then
    assert_ne!(lab.space_id(), ops.space_id());
    assert_ne!(lab.space_id(), unnamed.space_id());
    Ok(())
}

#[test]
fn two_spaces_may_share_one_name() -> TestResult {
    // Given
    let first = named(GENESIS_NONCE, "lab")?;

    // When
    let second = named([0x44; 32], "lab")?;

    // Then
    assert_eq!(first.name(), second.name());
    assert_ne!(first.space_id(), second.space_id());
    Ok(())
}

#[test]
fn a_genesis_without_a_name_keeps_its_exact_pre_name_encoding() -> TestResult {
    // Given
    let legacy = unnamed()?;

    // When
    let decoded = SignedSpaceGenesisV1::from_canonical_bytes(legacy.canonical_bytes())?;

    // Then
    assert_eq!(decoded.name(), None);
    assert_eq!(decoded, legacy);
    // A six-entry body is exactly what pre-name releases wrote and signed.
    assert_eq!(legacy.canonical_body_bytes().first(), Some(&0xa6));
    assert_eq!(
        named(GENESIS_NONCE, "lab")?.canonical_body_bytes().first(),
        Some(&0xa7)
    );
    Ok(())
}

#[test]
fn unusable_names_are_rejected_before_signing() -> TestResult {
    // Given
    let secret = SpaceAuthoritySecret::from_bytes(AUTHORITY_SECRET);
    let body = SpaceGenesisV1::new(
        SpaceGenesisIdentity::new(GENESIS_NONCE, 1, secret.public_key())?,
        SpaceGenesisOwner::new(owner()?, SpacePolicyV1::phase_one_default()),
    );

    // When / Then
    assert!(body.clone().with_name("").is_err());
    assert!(body.clone().with_name("lab\nops").is_err());
    assert!(
        body.clone()
            .with_name(&"n".repeat(MAX_SPACE_NAME_LEN + 1))
            .is_err()
    );
    assert!(body.with_name(&"n".repeat(MAX_SPACE_NAME_LEN)).is_ok());
    Ok(())
}

#[test]
fn a_name_appended_after_signing_does_not_verify() -> TestResult {
    // Given
    let named_genesis = named(GENESIS_NONCE, "lab")?;
    let mut forged = named_genesis.canonical_bytes().to_vec();

    // When: keep the signature but claim a different name of the same length.
    let position = forged
        .windows(3)
        .position(|window| window == b"lab")
        .ok_or("the encoded name is missing")?;
    forged
        .get_mut(position..position + 3)
        .ok_or("the encoded name is truncated")?
        .copy_from_slice(b"ops");

    // Then
    assert!(SignedSpaceGenesisV1::from_canonical_bytes(&forged).is_err());
    Ok(())
}
