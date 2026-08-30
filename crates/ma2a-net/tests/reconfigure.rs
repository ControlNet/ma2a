//! End-to-end live relay reconfiguration coverage.

#![allow(
    clippy::mod_module_files,
    reason = "shared integration test support is not an independent test target"
)]

mod support;

use iroh_base::SecretKey;
use ma2a_net::{
    AddressPublishRequest, EndpointSecret, LocalIrohRelayMap, PublicRelayFallbackConfig,
    RuntimeEndpoint,
};
use ma2a_store::{Repository, SpaceRecord, StoreConfig};
use support::{TempState, TestResult, space_fixture};

const NOW_MS: u64 = 1_700_000_000_000;

#[tokio::test]
async fn relay_mutation_preserves_identity_and_forces_next_actual_record() -> TestResult {
    // Given
    let secret_bytes = [0x61; 32];
    let secret = SecretKey::from_bytes(&secret_bytes);
    let fixture = space_fixture(&secret, 0x62)?;
    let state = TempState::new("connection-reconfigure")?;
    let mut repository = Repository::open(&StoreConfig::new(state.path()))?;
    repository.create_space(&SpaceRecord::new(
        fixture.genesis.space_id(),
        fixture.genesis.canonical_bytes().to_vec(),
    ))?;
    let (enrollment, _enrollment_rx) = tokio::sync::mpsc::channel(1);
    let mut endpoint =
        RuntimeEndpoint::bind(EndpointSecret::parse(&secret_bytes)?, enrollment, None).await?;
    let endpoint_id = endpoint.endpoint_id();
    let publisher = endpoint.address_publisher()?;
    let initial = publisher
        .publish(
            &mut repository,
            AddressPublishRequest::new(&fixture.authorization, NOW_MS),
        )?
        .ok_or("initial publication was skipped")?;
    let fallback = PublicRelayFallbackConfig::new(vec!["https://127.0.0.1:65530".parse()?])?;
    let map = LocalIrohRelayMap::from_control_spaces(&[], Some(&fallback), NOW_MS);

    // When
    let changed = endpoint.replace_relay_map(&map).await?;
    let updated = publisher
        .publish(
            &mut repository,
            AddressPublishRequest::new(&fixture.authorization, NOW_MS + 1).force_advance(),
        )?
        .ok_or("forced publication was skipped")?;

    // Then
    assert!(changed);
    assert_eq!(endpoint.endpoint_id(), endpoint_id);
    assert_eq!(updated.record().record().endpoint_id(), endpoint_id);
    assert_eq!(
        updated.record().record().sequence(),
        initial.record().record().sequence() + 1
    );
    endpoint.shutdown().await?;
    Ok(())
}
