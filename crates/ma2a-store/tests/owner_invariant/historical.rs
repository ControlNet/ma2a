use super::support::{TempState, TestResult};
use super::{authority_snapshot, member, peer_member};
use ma2a_core::{SpaceManifestMembership, SpacePolicyV1, SpaceRevocationV1};
use ma2a_store::{OwnedSpaceUpdate, Repository, SpaceCreation, StoreConfig};

#[test]
fn historical_owned_owner_removal_is_rejected_on_open() -> TestResult {
    let state = TempState::new("imported-owner-removal")?;
    let config = StoreConfig::new(state.path());
    let mut repository = Repository::open(&config)?;
    let owner = member()?;
    let peer = peer_member()?;
    let created = repository.create_owned_space(&SpaceCreation::new(
        1000,
        owner.clone(),
        SpacePolicyV1::phase_one_default(),
    ))?;
    let added = repository.advance_owned_space(&OwnedSpaceUpdate::new(
        created.space_id(),
        2000,
        SpaceManifestMembership::new(vec![peer.clone(), owner.clone()], vec![]),
    ))?;
    let invitation = repository.create_enrollment_invite(
        created.space_id(),
        owner.endpoint_id(),
        iroh_base::EndpointAddr::new(iroh_base::PublicKey::from_bytes(
            owner.endpoint_id().as_bytes(),
        )?),
        ma2a_core::InviteValidity::new(2000, 20_000)?,
        &ma2a_core::InviteEntropy::random()?,
    )?;
    // Adversarial signed-chain construction deliberately bypasses local signing.
    // Simulate a historical generation written before owner preservation existed.
    let (reference, bytes) = authority_snapshot(&config)?;
    let secret = ma2a_core::SpaceAuthoritySecret::try_from_bytes(&bytes)?;
    let mut chain = added.chain().clone();
    let manifest = ma2a_core::SpaceManifestV1::new(
        ma2a_core::SpaceManifestLink::new(created.space_id(), 2, chain.latest_hash()),
        3000,
        SpaceManifestMembership::new(
            vec![peer],
            vec![SpaceRevocationV1::new(owner.endpoint_id())],
        ),
    )?
    .sign(&secret)?;
    chain.apply(&manifest)?;
    // The production import boundary rejects an owner-invalid chain when the
    // repository owns its authority, before writing any row.
    let before = repository.revision()?;
    assert!(matches!(
        repository.persist_space_chain(&chain),
        Err(ma2a_store::StoreError::SchemaMismatch { .. })
    ));
    assert_eq!(repository.revision()?, before);
    // Test-only historical fixture: import as an external chain, then restore
    // the legacy authority reference directly in SQLite. Never repair it here.
    let sql = rusqlite::Connection::open(config.database_path())?;
    sql.execute("UPDATE spaces SET authority_key_ref = NULL", [])?;
    assert!(repository.persist_space_chain(&chain)?.error().is_none());
    let external = Repository::open(&config)?;
    assert_eq!(
        external.load_space_chain(created.space_id())?,
        Some(chain.clone())
    );
    drop(external);
    sql.execute("UPDATE spaces SET authority_key_ref = ?1", [&reference])?;
    let revision = repository.revision()?;
    assert!(repository.load_space_chain(created.space_id()).is_err());
    blocked_invites(&mut repository, &invitation, owner.endpoint_id())?;
    assert!(
        repository
            .advance_owned_space(&OwnedSpaceUpdate::new(
                created.space_id(),
                4000,
                SpaceManifestMembership::new(vec![owner], vec![]),
            ))
            .is_err()
    );
    assert_eq!(repository.revision()?, revision);
    assert!(authority_snapshot(&config)?.1 == bytes);
    drop(repository);
    assert!(matches!(
        Repository::open(&config),
        Err(ma2a_store::StoreError::SchemaMismatch { .. })
    ));
    Ok(())
}

fn blocked_invites(
    repository: &mut Repository,
    invite: &ma2a_store::CreatedEnrollmentInvite,
    owner: ma2a_core::EndpointId,
) -> TestResult {
    assert!(matches!(
        repository.create_enrollment_invite(
            invite.chain().space_id(),
            owner,
            iroh_base::EndpointAddr::new(iroh_base::PublicKey::from_bytes(owner.as_bytes())?),
            ma2a_core::InviteValidity::new(4000, 20_000)?,
            &ma2a_core::InviteEntropy::random()?,
        ),
        Err(ma2a_store::StoreError::SchemaMismatch { .. })
    ));
    let candidate = iroh_base::SecretKey::generate().public().into();
    let authorized = ma2a_store::AuthorizedEnrollmentRedemption::new(
        invite.ticket().clone(),
        ma2a_store::EnrollmentRedemption {
            endpoint_id: candidate,
            request_id: ma2a_core::RequestId::try_from([0x72; 16].as_slice())?,
            display_name: "candidate".to_owned(),
        },
        4000,
    );
    assert!(matches!(
        repository.redeem_enrollment(&authorized),
        Err(ma2a_store::StoreError::SchemaMismatch { .. })
    ));
    Ok(())
}
