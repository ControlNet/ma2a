//! Adversarial acceptance tests for signed Space manifest chains.

#[path = "support/space_vectors.rs"]
mod space_vectors;

use ma2a_core::{
    Capability, ManifestApplyOutcome, ManifestError, ProtocolError, SpaceAuthoritySecret,
    SpaceAuthorizationView, SpaceChain, SpaceGenesisIdentity, SpaceGenesisOwner, SpaceManifestLink,
    SpaceManifestMembership, SpaceManifestV1, SpaceRevocationV1, authorize_any,
};

use space_vectors::{member, signed_genesis, signed_manifest};

const SECOND_AUTHORITY_SECRET: [u8; 32] = [0x22; 32];

#[test]
fn replay_is_idempotent_only_for_identical_generation_and_hash()
-> Result<(), Box<dyn std::error::Error>> {
    let genesis = signed_genesis()?;
    let manifest = signed_manifest(&genesis)?;
    let mut chain = SpaceChain::from_genesis(genesis)?;
    assert_eq!(chain.apply(&manifest)?, ManifestApplyOutcome::ADVANCED);
    assert_eq!(chain.apply(&manifest)?, ManifestApplyOutcome::IDEMPOTENT);

    let other = SpaceManifestV1::new(
        SpaceManifestLink::new(chain.space_id(), 1, chain.genesis().chain_hash()),
        1_700_000_002_000,
        SpaceManifestMembership::new(vec![member(0x66, true)?], vec![]),
    )?
    .sign(&SpaceAuthoritySecret::from_bytes(
        space_vectors::AUTHORITY_SECRET,
    ))?;
    assert_eq!(chain.apply(&other), Err(ManifestError::FORK));
    Ok(())
}

#[test]
fn rollback_skip_wrong_link_and_wrong_signer_fail_closed() -> Result<(), Box<dyn std::error::Error>>
{
    let genesis = signed_genesis()?;
    let first = signed_manifest(&genesis)?;
    let mut chain = SpaceChain::from_genesis(genesis)?;
    chain.apply(&first)?;
    assert_eq!(chain.apply(&first), Ok(ManifestApplyOutcome::IDEMPOTENT));

    let secret = SpaceAuthoritySecret::from_bytes(space_vectors::AUTHORITY_SECRET);
    let skipped = SpaceManifestV1::new(
        SpaceManifestLink::new(chain.space_id(), 3, chain.latest_hash()),
        1_700_000_003_000,
        SpaceManifestMembership::new(first.members().to_vec(), vec![]),
    )?
    .sign(&secret)?;
    assert_eq!(
        chain.apply(&skipped),
        Err(ManifestError::SKIPPED_GENERATION)
    );

    let wrong_link = SpaceManifestV1::new(
        SpaceManifestLink::new(chain.space_id(), 2, [0x44; 32]),
        1_700_000_003_000,
        SpaceManifestMembership::new(first.members().to_vec(), vec![]),
    )?
    .sign(&secret)?;
    assert_eq!(chain.apply(&wrong_link), Err(ManifestError::FORK));

    let wrong_signer = SpaceManifestV1::new(
        SpaceManifestLink::new(chain.space_id(), 2, chain.latest_hash()),
        1_700_000_003_000,
        SpaceManifestMembership::new(first.members().to_vec(), vec![]),
    )?
    .sign(&SpaceAuthoritySecret::from_bytes(SECOND_AUTHORITY_SECRET))?;
    assert_eq!(
        chain.apply(&wrong_signer),
        Err(ManifestError::INVALID_SIGNATURE)
    );
    Ok(())
}

#[test]
fn removals_require_explicit_revocation_and_later_readdition_is_linked()
-> Result<(), Box<dyn std::error::Error>> {
    let genesis = signed_genesis()?;
    let first = signed_manifest(&genesis)?;
    let removed = space_vectors::endpoint(0x77)?;
    let secret = SpaceAuthoritySecret::from_bytes(space_vectors::AUTHORITY_SECRET);
    let mut chain = SpaceChain::from_genesis(genesis)?;
    chain.apply(&first)?;

    let implicit = SpaceManifestV1::new(
        SpaceManifestLink::new(chain.space_id(), 2, chain.latest_hash()),
        1_700_000_004_000,
        SpaceManifestMembership::new(vec![member(0x66, true)?], vec![]),
    )?
    .sign(&secret)?;
    assert_eq!(
        chain.apply(&implicit),
        Err(ManifestError::INVALID_REVOCATION)
    );

    let revoked = SpaceManifestV1::new(
        SpaceManifestLink::new(chain.space_id(), 2, chain.latest_hash()),
        1_700_000_004_000,
        SpaceManifestMembership::new(
            vec![member(0x66, true)?],
            vec![SpaceRevocationV1::new(removed)],
        ),
    )?
    .sign(&secret)?;
    assert_eq!(chain.apply(&revoked)?, ManifestApplyOutcome::ADVANCED);

    let dropped_revocation = SpaceManifestV1::new(
        SpaceManifestLink::new(chain.space_id(), 3, chain.latest_hash()),
        1_700_000_005_000,
        SpaceManifestMembership::new(vec![member(0x66, true)?], vec![]),
    )?
    .sign(&secret)?;
    assert_eq!(
        chain.apply(&dropped_revocation),
        Err(ManifestError::INVALID_REVOCATION)
    );

    let readded = SpaceManifestV1::new(
        SpaceManifestLink::new(chain.space_id(), 3, chain.latest_hash()),
        1_700_000_005_000,
        SpaceManifestMembership::new(vec![member(0x77, false)?, member(0x66, true)?], vec![]),
    )?
    .sign(&secret)?;
    assert_eq!(chain.apply(&readded)?, ManifestApplyOutcome::ADVANCED);
    Ok(())
}

#[test]
fn revoking_an_endpoint_absent_from_membership_history_is_rejected()
-> Result<(), Box<dyn std::error::Error>> {
    let genesis = signed_genesis()?;
    let absent = space_vectors::endpoint(0x77)?;
    let secret = SpaceAuthoritySecret::from_bytes(space_vectors::AUTHORITY_SECRET);
    let proposal = SpaceManifestV1::new(
        SpaceManifestLink::new(genesis.space_id(), 1, genesis.chain_hash()),
        1_700_000_001_000,
        SpaceManifestMembership::new(
            vec![member(0x66, true)?],
            vec![SpaceRevocationV1::new(absent)],
        ),
    )?
    .sign(&secret)?;
    let mut chain = SpaceChain::from_genesis(genesis)?;

    assert_eq!(
        chain.apply(&proposal),
        Err(ManifestError::INVALID_REVOCATION)
    );
    Ok(())
}

#[test]
fn malformed_unsorted_duplicate_and_broken_exports_fail_closed()
-> Result<(), Box<dyn std::error::Error>> {
    let genesis = signed_genesis()?;
    let manifest = signed_manifest(&genesis)?;
    assert!(
        SpaceManifestV1::new(
            SpaceManifestLink::new(genesis.space_id(), 1, genesis.chain_hash()),
            2,
            SpaceManifestMembership::new(vec![member(0x66, true)?, member(0x77, false)?], vec![],),
        )
        .is_err()
    );
    assert!(
        SpaceManifestV1::new(
            SpaceManifestLink::new(genesis.space_id(), 1, genesis.chain_hash()),
            2,
            SpaceManifestMembership::new(vec![member(0x66, true)?, member(0x66, false)?], vec![],),
        )
        .is_err()
    );

    let mut malformed = manifest.canonical_bytes().to_vec();
    malformed.push(0);
    assert_eq!(
        ma2a_core::SignedSpaceManifestV1::from_canonical_bytes(&malformed, genesis.authority()),
        Err(ProtocolError::INVALID_INPUT)
    );

    let mut chain = SpaceChain::from_genesis(genesis)?;
    chain.apply(&manifest)?;
    let mut export = chain.export_public()?;
    export.push(0);
    assert_eq!(
        SpaceChain::import_public(&export),
        Err(ManifestError::INVALID_ENCODING)
    );
    Ok(())
}

#[test]
fn revocation_is_space_local_and_cached_membership_has_no_lease()
-> Result<(), Box<dyn std::error::Error>> {
    let endpoint = space_vectors::endpoint(0x77)?;
    let genesis_a = signed_genesis()?;
    let mut chain_a = SpaceChain::from_genesis(genesis_a.clone())?;
    chain_a.apply(&signed_manifest(&genesis_a)?)?;

    let secret_b = SpaceAuthoritySecret::from_bytes(SECOND_AUTHORITY_SECRET);
    let genesis_b = ma2a_core::SpaceGenesisV1::new(
        SpaceGenesisIdentity::new([0x55; 32], 1, secret_b.public_key())?,
        SpaceGenesisOwner::new(
            member(0x77, false)?,
            ma2a_core::SpacePolicyV1::phase_one_default(),
        ),
    )
    .sign(&secret_b)?;
    let chain_b = SpaceChain::from_genesis(genesis_b)?;

    let view_a = SpaceAuthorizationView::from_chain(&chain_a);
    let cached_view_a = view_a.clone();
    let view_b = SpaceAuthorizationView::from_chain(&chain_b);
    assert!(authorize_any(
        &[view_a, view_b.clone()],
        endpoint,
        Capability::ECHO
    ));

    let secret_a = SpaceAuthoritySecret::from_bytes(space_vectors::AUTHORITY_SECRET);
    let revoke = SpaceManifestV1::new(
        SpaceManifestLink::new(chain_a.space_id(), 2, chain_a.latest_hash()),
        u64::MAX,
        SpaceManifestMembership::new(
            vec![member(0x66, true)?],
            vec![SpaceRevocationV1::new(endpoint)],
        ),
    )?
    .sign(&secret_a)?;
    chain_a.apply(&revoke)?;

    assert!(!SpaceAuthorizationView::from_chain(&chain_a).allows(endpoint, Capability::ECHO));
    assert!(cached_view_a.allows(endpoint, Capability::ECHO));
    assert!(view_b.allows(endpoint, Capability::ECHO));
    assert!(authorize_any(&[view_b], endpoint, Capability::ECHO));
    Ok(())
}
