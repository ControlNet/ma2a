use ma2a_core::{
    ControlArtifactKind, ControlArtifactV1, ControlPageV1, ControlRequestV1, ControlResponseV1,
    MemberCapabilities, SpaceManifestMembership, SpaceMemberV1, SpaceRevocationV1,
};
use ma2a_net::{EndpointBindOptions, EndpointSecret, RuntimeEndpoint, SpaceAddressLookup};
use ma2a_runtime::Runtime;
use ma2a_store::{OwnedSpaceUpdate, Repository};
use tokio::sync::mpsc;

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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn authenticated_peer_forwarding_an_invalid_artifact_preserves_high_water() -> TestResult {
    // Given
    let fixture = control_fixture().await?;
    let owner_config = fixture.owner_config();
    let candidate_config = fixture.candidate_config();
    let owner_port = Repository::open(&owner_config)?
        .endpoint_bind_port()?
        .ok_or("owner bind port missing")?;
    let owner_record = fixture
        .owner_records
        .first()
        .ok_or("owner address record missing")?;
    let page = ControlPageV1::new(
        fixture.shared_spaces[0],
        false,
        vec![
            ControlArtifactV1::new(
                ControlArtifactKind::ADDRESS_RECORD,
                owner_record.canonical_bytes().to_vec(),
            )?,
            ControlArtifactV1::new(ControlArtifactKind::RELAY_ADVERTISEMENT, vec![1])?,
        ],
    )?;
    let response = ControlResponseV1::new(vec![page])?.encode()?;
    let (enrollment_calls, _enrollment_receiver) = mpsc::channel(1);
    let (control_calls, mut control_receiver) = mpsc::channel(1);
    let malicious_owner = RuntimeEndpoint::bind_with_lookup(
        EndpointSecret::parse(&fixture.owner_secret.to_bytes())?,
        SpaceAddressLookup::default(),
        EndpointBindOptions::new(enrollment_calls, Some(owner_port))
            .with_control(control_calls, true),
    )
    .await?;
    let responder = tokio::spawn(async move {
        let call = control_receiver
            .recv()
            .await
            .ok_or("control call missing")?;
        call.respond(Ok(response));
        Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
    });
    let candidate = Runtime::start_with_clock(candidate_config.clone(), clock()).await?;
    let owner_id = fixture.owner_secret.public().into();
    let revision = Repository::open(&candidate_config)?.revision()?;

    // When
    let result = candidate.handle().sync_control().await;

    // Then
    assert!(result.is_err());
    responder.await??;
    let repository = Repository::open(&candidate_config)?;
    assert_eq!(repository.revision()?, revision);
    assert_eq!(
        repository
            .address_record(fixture.shared_spaces[0], owner_id)?
            .ok_or("owner address missing")?
            .sequence(),
        1
    );
    candidate.shutdown().await?;
    malicious_owner.shutdown().await?;
    Ok(())
}
