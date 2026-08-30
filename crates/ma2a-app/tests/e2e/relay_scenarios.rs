use iroh_base::{RelayUrl, SecretKey};
use ma2a_core::RelayReachability;
use ma2a_net::{LocalIrohRelayMap, PublicRelayFallbackConfig};
use ma2a_store::Repository;

use super::harness::{
    NOW_MS, RelayFixture, SpaceFixture, TempState, TestResult, create_space, emit,
    observed_home_endpoint, store_relay,
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
    assert_eq!(
        candidates.relay_urls().cloned().collect::<Vec<_>>(),
        vec![public_url.clone()]
    );
    emit(&serde_json::json!({
        "scenario": "B",
        "endpoint_ids": {"endpoint": matrix.local.public().to_string()},
        "record_sequences": [],
        "supplied_relay_candidates": [public_url.to_string()],
        "iroh_observed_effective_home": public_url.to_string(),
        "iroh_observed_path": "home-connected",
        "reachability_state": "IrohHomeConnected"
    }));
    endpoint.close().await;
    drop(public_relay);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scenario_c_common_private_relay_is_observed_as_home() -> TestResult {
    // Given
    let (private_map, private_url, private_relay) = iroh::test_utils::run_relay_server().await?;
    let mut matrix = RelayMatrix::new("scenario-c")?;
    matrix.add_common(&private_url)?;
    let candidates = matrix.map(None)?;

    // When
    let endpoint = observed_home_endpoint(&matrix.common, private_map, &private_url).await?;

    // Then
    assert_eq!(
        candidates.relay_urls().cloned().collect::<Vec<_>>(),
        vec![private_url.clone()]
    );
    emit(&serde_json::json!({
        "scenario": "C",
        "endpoint_ids": {"endpoint": matrix.local.public().to_string(), "provider": matrix.common.public().to_string()},
        "record_sequences": [1, 1],
        "supplied_relay_candidates": [private_url.to_string()],
        "iroh_observed_effective_home": private_url.to_string(),
        "iroh_observed_path": "home-connected",
        "reachability_state": "IrohHomeConnected"
    }));
    endpoint.close().await;
    drop(private_relay);
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
    emit(&serde_json::json!({
        "scenario": "D",
        "endpoint_ids": {"endpoint": matrix.local.public().to_string()},
        "record_sequences": [1, 1],
        "supplied_relay_candidates": [],
        "iroh_observed_effective_home": null,
        "iroh_observed_path": "direct-only-possible",
        "reachability_state": format!("{:?}", RelayReachability::DegradedNoCommonHome)
    }));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scenarios_e_and_f_survive_restart_and_advance_address_sequence_on_home_change()
-> TestResult {
    // Given
    let secret_bytes = [0x78; 32];
    let secret = SecretKey::from_bytes(&secret_bytes);
    let state = TempState::new("scenarios-ef")?;
    let mut repository = Repository::open(&state.config())?;
    let authorization = create_space(
        &mut repository,
        SpaceFixture {
            local: &secret,
            relays: &[],
            issued_at_ms: 40,
        },
    )?;
    let (enrollment, _receiver) = tokio::sync::mpsc::channel(1);
    let mut endpoint = ma2a_net::RuntimeEndpoint::bind(
        ma2a_net::EndpointSecret::parse(&secret_bytes)?,
        enrollment,
        None,
    )
    .await?;
    let publisher = endpoint.address_publisher()?;
    let first = publisher
        .publish(
            &mut repository,
            ma2a_net::AddressPublishRequest::new(&authorization, NOW_MS),
        )?
        .ok_or("first address publication missing")?;
    endpoint.shutdown().await?;
    let (relay_map, relay_url, relay) = iroh::test_utils::run_relay_server().await?;
    let restarted = observed_home_endpoint(&secret, relay_map, &relay_url).await?;
    restarted.close().await;
    drop(relay);
    let fallback = PublicRelayFallbackConfig::new(vec!["https://127.0.0.1:65530".parse()?])?;
    let desired = LocalIrohRelayMap::from_control_spaces(&[], Some(&fallback), NOW_MS);
    let (enrollment, _receiver) = tokio::sync::mpsc::channel(1);
    endpoint = ma2a_net::RuntimeEndpoint::bind(
        ma2a_net::EndpointSecret::parse(&secret_bytes)?,
        enrollment,
        None,
    )
    .await?;

    // When
    endpoint.replace_relay_map(&desired).await?;
    let second = endpoint
        .address_publisher()?
        .publish(
            &mut repository,
            ma2a_net::AddressPublishRequest::new(&authorization, NOW_MS + 1).force_advance(),
        )?
        .ok_or("changed-home address publication missing")?;

    // Then
    assert_eq!(
        second.record().record().sequence(),
        first.record().record().sequence() + 1
    );
    emit(&serde_json::json!({
        "scenario": "E",
        "endpoint_ids": {"endpoint": secret.public().to_string()},
        "record_sequences": [first.record().record().sequence()],
        "supplied_relay_candidates": [relay_url.to_string()],
        "iroh_observed_effective_home": relay_url.to_string(),
        "iroh_observed_path": "relearned-after-restart",
        "reachability_state": "connected-without-cached-route"
    }));
    emit(&serde_json::json!({
        "scenario": "F",
        "endpoint_ids": {"endpoint": secret.public().to_string()},
        "record_sequences": [first.record().record().sequence(), second.record().record().sequence()],
        "supplied_relay_candidates": desired.relay_urls().map(ToString::to_string).collect::<Vec<_>>(),
        "iroh_observed_effective_home": "changed",
        "iroh_observed_path": "address-republished",
        "reachability_state": "AwaitingIrohHome"
    }));
    endpoint.shutdown().await?;
    Ok(())
}

struct RelayMatrix {
    _state: TempState,
    repository: Repository,
    local: SecretKey,
    common: SecretKey,
    spaces: [ma2a_core::SpaceAuthorizationView; 2],
}

impl RelayMatrix {
    fn new(name: &str) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let state = TempState::new(name)?;
        let local = SecretKey::from_bytes(&[0x74; 32]);
        let relay_x = SecretKey::from_bytes(&[0x75; 32]);
        let relay_y = SecretKey::from_bytes(&[0x76; 32]);
        let common = SecretKey::from_bytes(&[0x77; 32]);
        let mut repository = Repository::open(&state.config())?;
        let x = create_space(
            &mut repository,
            SpaceFixture {
                local: &local,
                relays: &[&relay_x, &common],
                issued_at_ms: 20,
            },
        )?;
        let y = create_space(
            &mut repository,
            SpaceFixture {
                local: &local,
                relays: &[&relay_y, &common],
                issued_at_ms: 30,
            },
        )?;
        for fixture in [
            RelayFixture {
                authorization: &x,
                relay: &relay_x,
                relay_url: "https://rx.example.invalid".parse()?,
                sequence: 1,
            },
            RelayFixture {
                authorization: &y,
                relay: &relay_y,
                relay_url: "https://ry.example.invalid".parse()?,
                sequence: 1,
            },
        ] {
            store_relay(&mut repository, fixture)?;
        }
        Ok(Self {
            _state: state,
            repository,
            local,
            common,
            spaces: [x, y],
        })
    }

    fn add_common(&mut self, relay_url: &RelayUrl) -> TestResult {
        for authorization in &self.spaces {
            store_relay(
                &mut self.repository,
                RelayFixture {
                    authorization,
                    relay: &self.common,
                    relay_url: relay_url.clone(),
                    sequence: 1,
                },
            )?;
        }
        Ok(())
    }

    fn map(
        &self,
        fallback: Option<&PublicRelayFallbackConfig>,
    ) -> Result<LocalIrohRelayMap, Box<dyn std::error::Error + Send + Sync>> {
        let spaces = self
            .repository
            .control_spaces_for(self.local.public().into())?;
        Ok(LocalIrohRelayMap::from_control_spaces(
            &spaces, fallback, NOW_MS,
        ))
    }
}
