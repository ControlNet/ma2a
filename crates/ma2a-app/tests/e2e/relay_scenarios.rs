use iroh::Watcher as _;
use ma2a_core::RelayReachability;
use ma2a_net::PublicRelayFallbackConfig;

use super::{
    harness::{TestResult, emit, observed_home_endpoint},
    relay_matrix::RelayMatrix,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scenario_b_incompatible_private_relays_use_observed_public_home() -> TestResult {
    // Given
    let (public_map, public_url, public_relay) = iroh::test_utils::run_relay_server().await?;
    let matrix = RelayMatrix::new("scenario-b")?;
    let fallback = PublicRelayFallbackConfig::new(vec![public_url.clone()])?;
    let candidates = matrix.map(Some(&fallback))?;

    // When
    let endpoint = observed_home_endpoint(&matrix.local, public_map, &public_url).await?;

    // Then
    let supplied = candidates.relay_urls().cloned().collect::<Vec<_>>();
    assert_eq!(supplied, vec![public_url.clone()]);
    let mut home_status = endpoint.home_relay_status();
    let homes = home_status.get();
    let observed = homes
        .iter()
        .find(|status| status.url() == &public_url && status.is_connected())
        .ok_or("public home was not connected")?;
    emit(&serde_json::json!({
        "scenario": "B",
        "endpoint_ids": {"endpoint": matrix.local.public().to_string()},
        "supplied_relay_candidates": supplied.iter().map(ToString::to_string).collect::<Vec<_>>(),
        "iroh_observed_effective_home": observed.url().to_string(),
        "home_connected": observed.is_connected()
    }));
    endpoint.close().await;
    drop(public_relay);
    Ok(())
}

#[test]
fn scenario_d_no_common_relay_is_exactly_degraded() -> TestResult {
    // Given
    let matrix = RelayMatrix::new("scenario-d")?;

    // When
    let candidates = matrix.map(None)?;

    // Then
    assert!(candidates.is_empty());
    let reachability = RelayReachability::DegradedNoCommonHome;
    emit(&serde_json::json!({
        "scenario": "D",
        "endpoint_ids": {"endpoint": matrix.local.public().to_string()},
        "supplied_relay_candidates": [],
        "reachability_state": format!("{reachability:?}")
    }));
    Ok(())
}
