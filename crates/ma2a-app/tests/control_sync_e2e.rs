//! End-to-end active-dial control synchronization coverage.

#[path = "control_sync_e2e/adversarial.rs"]
mod control_sync_e2e_adversarial;
#[path = "control_sync_e2e/artifacts.rs"]
mod control_sync_e2e_artifacts;
#[path = "control_sync_e2e/support.rs"]
mod control_sync_e2e_support;

use std::time::Duration;

use ma2a_core::{ControlRequestV1, ControlResponseV1, SpaceManifestMembership, SpaceRevocationV1};
use ma2a_net::{EndpointSecret, RuntimeEndpoint};
use ma2a_runtime::Runtime;
use ma2a_store::{OwnedSpaceUpdate, Repository};
use tokio::sync::mpsc;

use control_sync_e2e_support::{TestResult, clock, control_fixture};

type TestResultValue<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn explicit_round_converges_shared_spaces_without_leaking_private_space() -> TestResult {
    // Given
    let fixture = control_fixture().await?;
    let owner_config = fixture.owner_config();
    let candidate_config = fixture.candidate_config();
    let owner = Runtime::start_with_clock(owner_config.clone(), clock()).await?;
    let candidate = Runtime::start_with_clock(candidate_config.clone(), clock()).await?;

    // When
    tokio::time::timeout(Duration::from_secs(15), candidate.handle().sync_control()).await??;

    // Then
    let candidate_repository = Repository::open(&candidate_config)?;
    let owner_repository = Repository::open(&owner_config)?;
    for (((space_id, candidate_record), owner_record), owner_advertisement) in fixture
        .shared_spaces
        .into_iter()
        .zip(&fixture.candidate_records)
        .zip(&fixture.owner_records)
        .zip(&fixture.owner_advertisements)
    {
        let synchronized = candidate_repository
            .load_space_chain(space_id)?
            .ok_or("candidate chain missing")?;
        assert_eq!(synchronized.latest_generation(), 2);
        let imported = owner_repository
            .address_record(space_id, fixture.candidate_secret.public().into())?
            .ok_or("candidate address was not pushed")?;
        assert_eq!(imported.signed_record(), candidate_record.canonical_bytes());
        let imported_owner = candidate_repository
            .address_record(space_id, fixture.owner_secret.public().into())?
            .ok_or("newer owner address was not pulled")?;
        assert_eq!(
            imported_owner.signed_record(),
            owner_record.canonical_bytes()
        );
        let imported_relay = candidate_repository
            .relay_advertisement(space_id, fixture.owner_secret.public().into())?
            .ok_or("owner relay advertisement was not pulled")?;
        assert_eq!(
            imported_relay.signed_advertisement(),
            owner_advertisement.canonical_bytes()
        );
    }
    assert!(
        candidate_repository
            .load_space_chain(fixture.private_space)?
            .is_none()
    );
    candidate.shutdown().await?;
    owner.shutdown().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn malformed_control_request_is_rejected_without_revision_change() -> TestResult {
    // Given
    let fixture = control_fixture().await?;
    let owner_config = fixture.owner_config();
    let owner = Runtime::start_with_clock(owner_config.clone(), clock()).await?;
    let owner_addr = owner.handle().status().await?.endpoint_addr().clone();
    let revision = Repository::open(&owner_config)?.revision()?;
    let client = raw_endpoint(&fixture.candidate_secret).await?;

    // When
    let result = client
        .exchange_control(owner_addr, b"not-a-control-request")
        .await;

    // Then
    assert!(result.is_err());
    assert_eq!(Repository::open(&owner_config)?.revision()?, revision);
    client.shutdown().await?;
    owner.shutdown().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn revoked_space_is_filtered_while_another_shared_space_remains_available() -> TestResult {
    // Given
    let fixture = control_fixture().await?;
    let owner_config = fixture.owner_config();
    let candidate_config = fixture.candidate_config();
    let revoked_space = fixture.shared_spaces[0];
    let retained_space = fixture.shared_spaces[1];
    let candidate_id = fixture.candidate_secret.public().into();
    let mut owner_repository = Repository::open(&owner_config)?;
    let revoked_chain = owner_repository
        .load_space_chain(revoked_space)?
        .ok_or("revoked Space chain missing")?;
    let retained_members = revoked_chain
        .members()
        .iter()
        .filter(|member| member.endpoint_id() != candidate_id)
        .cloned()
        .collect();
    owner_repository.advance_owned_space(&OwnedSpaceUpdate::new(
        revoked_space,
        1_700_000_000_100,
        SpaceManifestMembership::new(retained_members, vec![SpaceRevocationV1::new(candidate_id)]),
    ))?;
    drop(owner_repository);
    let candidate_repository = Repository::open(&candidate_config)?;
    let retained_cursor = candidate_repository
        .control_spaces_for(candidate_id)?
        .into_iter()
        .find(|state| state.chain().space_id() == retained_space)
        .ok_or("retained shared Space missing")?
        .cursor()?;
    let request = ControlRequestV1::new(vec![retained_cursor])?.encode()?;
    let owner = Runtime::start_with_clock(owner_config, clock()).await?;
    let owner_addr = owner.handle().status().await?.endpoint_addr().clone();
    let client = raw_endpoint(&fixture.candidate_secret).await?;

    // When
    let response = client.exchange_control(owner_addr, &request).await?;
    let response = ControlResponseV1::decode(&response)?;

    // Then
    assert!(
        response
            .pages()
            .iter()
            .all(|page| page.space_id() == retained_space)
    );
    assert!(
        response
            .pages()
            .iter()
            .all(|page| page.space_id() != revoked_space)
    );
    client.shutdown().await?;
    owner.shutdown().await?;
    Ok(())
}

async fn raw_endpoint(secret: &iroh::SecretKey) -> TestResultValue<RuntimeEndpoint> {
    let (enrollment_calls, _receiver) = mpsc::channel(1);
    Ok(RuntimeEndpoint::bind(
        EndpointSecret::parse(&secret.to_bytes())?,
        enrollment_calls,
        None,
    )
    .await?)
}
