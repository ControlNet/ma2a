use iroh::Watcher as _;
use ma2a_core::{
    MemberCapabilities, RelayReachability, SpaceManifestMembership, SpaceMemberV1, SpacePolicyV1,
};
use ma2a_net::PublicRelayFallbackConfig;
use ma2a_runtime::Runtime;
use ma2a_store::{OwnedSpaceUpdate, Repository, SpaceCreation};

use super::{
    harness::{TestResult, emit, observed_home_endpoint},
    relay_matrix::RelayMatrix,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scenario_b_incompatible_private_relays_use_observed_public_home() -> TestResult {
    // Given
    let (_public_map, public_url, public_relay) = iroh::test_utils::run_relay_server().await?;
    let matrix = RelayMatrix::new("scenario-b")?;
    let fallback = PublicRelayFallbackConfig::new(vec![public_url.clone()])?;
    let candidates = matrix.map(Some(&fallback))?;

    // When
    let endpoint =
        observed_home_endpoint(&matrix.local, candidates.relay_map(), &public_url).await?;

    // Then
    let supplied = candidates.relay_urls().cloned().collect::<Vec<_>>();
    assert_eq!(supplied, vec![public_url.clone()]);
    assert_eq!(candidates.active_space_count(), 2);
    assert_eq!(candidates.private_home_relays().count(), 0);
    assert_eq!(
        candidates.public_relays(),
        std::slice::from_ref(&public_url)
    );
    let mut home_status = endpoint.home_relay_status();
    let homes = home_status.get();
    let observed = homes
        .iter()
        .find(|status| status.url() == &public_url && status.is_connected())
        .ok_or("public home was not connected")?;
    emit(&serde_json::json!({
        "scenario": "B",
        "endpoint_ids": {"endpoint": matrix.local.public().to_string()},
        "record_sequences": [],
        "supplied_relay_candidates": supplied.iter().map(ToString::to_string).collect::<Vec<_>>(),
        "iroh_observed_effective_home": observed.url().to_string(),
        "iroh_observed_path": null,
        "reachability_state": "PublicHomeConnected",
        "home_connected": observed.is_connected()
    }));
    endpoint.close().await;
    drop(public_relay);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scenario_d_no_common_relay_is_exactly_degraded() -> TestResult {
    // Given
    let state = super::harness::TempState::new("scenario-d")?;
    let config = state.config();
    let bootstrap = Runtime::start(config.clone()).await?;
    let endpoint_id = bootstrap.handle().status().await?.endpoint_id();
    bootstrap.shutdown().await?;
    let mut repository = Repository::open(&config)?;
    create_local_space(&mut repository, endpoint_id, 90)?;
    create_local_space(&mut repository, endpoint_id, 100)?;
    drop(repository);

    // When
    let runtime = Runtime::start(config).await?;
    let status = runtime.handle().status().await?;

    // Then
    assert_eq!(status.membership_count(), 2);
    assert_eq!(
        status.relay_reachability(),
        RelayReachability::DegradedNoCommonHome
    );
    assert!(status.observed_home_relays().next().is_none());
    emit(&serde_json::json!({
        "scenario": "D",
        "endpoint_ids": {"endpoint": endpoint_id.to_public_key()?.to_string()},
        "record_sequences": [],
        "supplied_relay_candidates": [],
        "iroh_observed_effective_home": null,
        "iroh_observed_path": null,
        "reachability_state": format!("{:?}", status.relay_reachability())
    }));
    runtime.shutdown().await?;
    Ok(())
}

fn create_local_space(
    repository: &mut Repository,
    endpoint_id: ma2a_core::EndpointId,
    issued_at_ms: u64,
) -> TestResult {
    let member = SpaceMemberV1::new(
        endpoint_id,
        format!("local-{issued_at_ms}"),
        MemberCapabilities::new(true, false),
    )?;
    let created = repository.create_owned_space(&SpaceCreation::new(
        issued_at_ms,
        member.clone(),
        SpacePolicyV1::phase_one_default(),
    ))?;
    repository.advance_owned_space(&OwnedSpaceUpdate::new(
        created.space_id(),
        issued_at_ms + 1,
        SpaceManifestMembership::new(vec![member], Vec::new()),
    ))?;
    Ok(())
}
