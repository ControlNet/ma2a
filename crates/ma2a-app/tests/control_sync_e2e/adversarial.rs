use ma2a_core::{
    ControlRequestV1, MemberCapabilities, SpaceManifestMembership, SpaceMemberV1, SpaceRevocationV1,
};
use ma2a_runtime::Runtime;
use ma2a_store::{OwnedSpaceUpdate, Repository};

#[path = "adversarial/forwarding.rs"]
mod forwarding;

use super::{
    control_sync_e2e_support::{TestResult, clock, control_fixture},
    raw_endpoint,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn learned_revocation_denies_the_revoked_endpoint_over_live_iroh() -> TestResult {
    // Given
    let fixture = control_fixture().await?;
    let owner_config = fixture.owner_config();
    let candidate_config = fixture.candidate_config();
    let space_id = fixture.shared_spaces[0];
    let revoked_secret = iroh::SecretKey::from_bytes(&[0x79; 32]);
    let revoked_id = revoked_secret.public().into();
    let mut owner_repository = Repository::open(&owner_config)?;
    let chain = owner_repository
        .load_space_chain(space_id)?
        .ok_or("shared Space chain missing")?;
    let mut members = chain.members().to_vec();
    members.push(SpaceMemberV1::new(
        revoked_id,
        "revoked peer".to_owned(),
        MemberCapabilities::new(true, true),
    )?);
    members.sort_by_key(SpaceMemberV1::endpoint_id);
    owner_repository.advance_owned_space(&OwnedSpaceUpdate::new(
        space_id,
        1_700_000_000_100,
        SpaceManifestMembership::new(members, vec![]),
    ))?;
    drop(owner_repository);
    let owner = Runtime::start_with_clock(owner_config.clone(), clock()).await?;
    let candidate = Runtime::start_with_clock(candidate_config.clone(), clock()).await?;
    candidate.handle().sync_control().await?;
    let chain = Repository::open(&owner_config)?
        .load_space_chain(space_id)?
        .ok_or("advanced shared Space chain missing")?;
    let retained = chain
        .members()
        .iter()
        .filter(|member| member.endpoint_id() != revoked_id)
        .cloned()
        .collect();
    owner
        .handle()
        .advance_owned_space(OwnedSpaceUpdate::new(
            space_id,
            1_700_000_000_200,
            SpaceManifestMembership::new(retained, vec![SpaceRevocationV1::new(revoked_id)]),
        ))
        .await?;
    candidate.handle().sync_control().await?;
    let candidate_addr = candidate.handle().status().await?.endpoint_addr().clone();
    let revision = Repository::open(&candidate_config)?.revision()?;
    let revoked = raw_endpoint(&revoked_secret).await?;
    let request = ControlRequestV1::new(Vec::new())?.encode()?;

    // When
    let denied = revoked.exchange_control(candidate_addr, &request).await;

    // Then
    assert!(denied.is_err());
    assert_eq!(Repository::open(&candidate_config)?.revision()?, revision);
    revoked.shutdown().await?;
    candidate.shutdown().await?;
    owner.shutdown().await?;
    Ok(())
}
