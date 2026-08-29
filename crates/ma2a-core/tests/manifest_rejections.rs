//! Rejection-state tests for signed Space manifest chains.

#[path = "support/space_vectors.rs"]
mod space_vectors;

use ma2a_core::{
    ManifestApplyOutcome, ManifestError, ProtocolError, SignedSpaceManifestV1,
    SpaceAuthoritySecret, SpaceChain, SpaceManifestLink, SpaceManifestMembership, SpaceManifestV1,
    SpaceRevocationV1,
};
use space_vectors::{member, signed_genesis, signed_manifest};

#[test]
fn older_applied_generation_is_rollback_and_preserves_latest_state()
-> Result<(), Box<dyn std::error::Error>> {
    let genesis = signed_genesis()?;
    let first = signed_manifest(&genesis)?;
    let secret = SpaceAuthoritySecret::from_bytes(space_vectors::AUTHORITY_SECRET);
    let mut chain = SpaceChain::from_genesis(genesis)?;
    chain.apply(&first)?;
    let second = SpaceManifestV1::new(
        SpaceManifestLink::new(chain.space_id(), 2, chain.latest_hash()),
        1_700_000_002_000,
        SpaceManifestMembership::new(first.members().to_vec(), vec![]),
    )?
    .sign(&secret)?;
    chain.apply(&second)?;
    let latest_hash = chain.latest_hash();
    let members = chain.members().to_vec();

    assert_eq!(chain.apply(&first), Err(ManifestError::ROLLBACK));
    assert_eq!(chain.latest_generation(), 2);
    assert_eq!(chain.latest_hash(), latest_hash);
    assert_eq!(chain.members(), members);
    Ok(())
}

#[test]
fn authority_swap_cannot_verify_an_existing_manifest() -> Result<(), Box<dyn std::error::Error>> {
    let genesis = signed_genesis()?;
    let manifest = signed_manifest(&genesis)?;
    let other_authority = SpaceAuthoritySecret::from_bytes([0x22; 32]).public_key();

    assert_eq!(
        SignedSpaceManifestV1::from_canonical_bytes(manifest.canonical_bytes(), other_authority),
        Err(ProtocolError::INVALID_INPUT)
    );
    Ok(())
}

#[test]
fn revocations_must_be_sorted_unique_and_disjoint_from_members()
-> Result<(), Box<dyn std::error::Error>> {
    let genesis = signed_genesis()?;
    let lower = space_vectors::endpoint(0x77)?;
    let higher = space_vectors::endpoint(0x66)?;

    assert!(
        SpaceManifestV1::new(
            SpaceManifestLink::new(genesis.space_id(), 1, genesis.chain_hash()),
            1,
            SpaceManifestMembership::new(
                vec![member(0x66, true)?],
                vec![
                    SpaceRevocationV1::new(higher),
                    SpaceRevocationV1::new(lower),
                ],
            ),
        )
        .is_err()
    );
    assert!(
        SpaceManifestV1::new(
            SpaceManifestLink::new(genesis.space_id(), 1, genesis.chain_hash()),
            1,
            SpaceManifestMembership::new(
                vec![member(0x66, true)?],
                vec![SpaceRevocationV1::new(lower), SpaceRevocationV1::new(lower)],
            ),
        )
        .is_err()
    );
    assert!(
        SpaceManifestV1::new(
            SpaceManifestLink::new(genesis.space_id(), 1, genesis.chain_hash()),
            1,
            SpaceManifestMembership::new(
                vec![member(0x66, true)?],
                vec![SpaceRevocationV1::new(higher)],
            ),
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn nonminimal_signed_object_lengths_are_rejected() -> Result<(), Box<dyn std::error::Error>> {
    let genesis = signed_genesis()?;
    let manifest = signed_manifest(&genesis)?;
    let canonical = manifest.canonical_bytes();
    let mut nonminimal = Vec::with_capacity(canonical.len() + 1);
    nonminimal.extend_from_slice(canonical.get(..2).ok_or("missing envelope prefix")?);
    nonminimal.extend_from_slice(&[0x59, 0x00, *canonical.get(3).ok_or("missing body length")?]);
    nonminimal.extend_from_slice(canonical.get(4..).ok_or("missing envelope body")?);

    assert_eq!(
        SignedSpaceManifestV1::from_canonical_bytes(&nonminimal, genesis.authority()),
        Err(ProtocolError::INVALID_INPUT)
    );
    let mut wrong_key = canonical.to_vec();
    *wrong_key.get_mut(1).ok_or("missing first map key")? = 1;
    assert_eq!(
        SignedSpaceManifestV1::from_canonical_bytes(&wrong_key, genesis.authority()),
        Err(ProtocolError::INVALID_INPUT)
    );
    Ok(())
}

#[test]
fn exact_linear_advancement_remains_idempotent() -> Result<(), Box<dyn std::error::Error>> {
    let genesis = signed_genesis()?;
    let manifest = signed_manifest(&genesis)?;
    let mut chain = SpaceChain::from_genesis(genesis)?;
    assert_eq!(chain.apply(&manifest)?, ManifestApplyOutcome::ADVANCED);
    assert_eq!(chain.apply(&manifest)?, ManifestApplyOutcome::IDEMPOTENT);
    Ok(())
}
