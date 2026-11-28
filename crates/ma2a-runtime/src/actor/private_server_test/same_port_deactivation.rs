use super::{
    super::partial_relay_publication_test::activity_reconciliation_test::{
        add_peer, respond_has_relay_advertisement,
    },
    TempState, TestResult, tick,
};
use crate::{
    Runtime,
    actor::{maintenance::FaultPoint, relay_lifecycle_test::enabled_configuration},
    error::RuntimeError,
};
use ma2a_store::{Repository, StoreConfig, StoreError};

#[tokio::test(start_paused = true)]
async fn failed_same_port_replacement_deactivates_advertisements_immediately() -> TestResult {
    let state = TempState::new("same-port-deactivate")?;
    let config = StoreConfig::new(&state.0);
    let runtime = Runtime::start(config.clone()).await?;
    let handle = runtime.handle();
    let endpoint = handle.status().await?.endpoint_id();
    let space = handle
        .create_owned_space("same-port-deactivate".to_owned())
        .await?;
    let peer_id = ma2a_core::EndpointId::from(iroh::SecretKey::from_bytes(&[0x45; 32]).public());
    add_peer(&config, space, peer_id)?;

    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    drop(listener);
    let mut desired = enabled_configuration(vec![space]);
    desired.listener_address = Some(address.to_string());
    handle.set_relay_configuration(desired.clone()).await?;

    let initial_ad = Repository::open(&config)?
        .relay_advertisement(space, endpoint)?
        .ok_or("initial advertisement missing")?;
    assert!(initial_ad.is_active());
    let now_ms = u64::try_from(initial_ad.issued_at_ms())?;
    assert!(respond_has_relay_advertisement(
        &config,
        (endpoint, peer_id, space),
        now_ms
    )?);

    desired.private_relay_url = Some("https://replacement.example.invalid/".to_owned());
    handle.fail_background_once(
        FaultPoint::PrivateRelayStart,
        RuntimeError::from(StoreError::Io(std::io::ErrorKind::Interrupted.into())),
    );
    assert!(
        handle
            .set_relay_configuration(desired.clone())
            .await
            .is_err()
    );

    let status = handle.relay_status().await?;
    assert!(status.convergence_pending);
    assert!(status.applied_private.is_none());
    assert_eq!(status.configuration, desired);
    let after_fail = Repository::open(&config)?
        .relay_advertisement(space, endpoint)?
        .ok_or("persisted advertisement disappeared")?;
    assert!(
        !after_fail.is_active(),
        "advertisement remained active after failed same-port replacement"
    );
    assert!(!respond_has_relay_advertisement(
        &config,
        (endpoint, peer_id, space),
        now_ms
    )?);
    assert_eq!(
        handle
            .snapshot()
            .await?
            .to_value()
            .pointer("/private_relay_candidates"),
        Some(&serde_json::json!([]))
    );

    tick(&handle).await;
    assert_eq!(
        handle.relay_status().await?.private_listen_addr,
        Some(address)
    );
    assert!(!handle.relay_status().await?.convergence_pending);
    let recovered = Repository::open(&config)?
        .relay_advertisement(space, endpoint)?
        .ok_or("recovered advertisement missing")?;
    assert!(recovered.is_active());
    assert!(recovered.sequence() > initial_ad.sequence());
    assert!(respond_has_relay_advertisement(
        &config,
        (endpoint, peer_id, space),
        u64::try_from(recovered.issued_at_ms())?
    )?);
    runtime.shutdown().await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn failed_different_listener_replacement_keeps_advertisement_active() -> TestResult {
    let state = TempState::new("diff-port-keep-active")?;
    let config = StoreConfig::new(&state.0);
    let runtime = Runtime::start(config.clone()).await?;
    let handle = runtime.handle();
    let endpoint = handle.status().await?.endpoint_id();
    let space = handle
        .create_owned_space("diff-port-keep-active".to_owned())
        .await?;
    let peer_id = ma2a_core::EndpointId::from(iroh::SecretKey::from_bytes(&[0x46; 32]).public());
    add_peer(&config, space, peer_id)?;

    let mut desired = enabled_configuration(vec![space]);
    handle.set_relay_configuration(desired.clone()).await?;
    let initial_ad = Repository::open(&config)?
        .relay_advertisement(space, endpoint)?
        .ok_or("initial advertisement missing")?;
    assert!(initial_ad.is_active());
    let now_ms = u64::try_from(initial_ad.issued_at_ms())?;
    assert!(respond_has_relay_advertisement(
        &config,
        (endpoint, peer_id, space),
        now_ms
    )?);

    let occupied = std::net::TcpListener::bind("127.0.0.1:0")?;
    desired.listener_address = Some(occupied.local_addr()?.to_string());
    assert!(handle.set_relay_configuration(desired).await.is_err());

    let status = handle.relay_status().await?;
    assert!(status.applied_private.is_some());
    let after_fail = Repository::open(&config)?
        .relay_advertisement(space, endpoint)?
        .ok_or("advertisement disappeared")?;
    assert!(after_fail.is_active());
    assert!(respond_has_relay_advertisement(
        &config,
        (endpoint, peer_id, space),
        now_ms
    )?);

    drop(occupied);
    runtime.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn ephemeral_to_assigned_port_reconfiguration_stops_old_listener_first() -> TestResult {
    let state = TempState::new("ephemeral-to-fixed")?;
    let runtime = Runtime::start(StoreConfig::new(&state.0)).await?;
    let handle = runtime.handle();
    let space = handle
        .create_owned_space("ephemeral-to-fixed".to_owned())
        .await?;

    let mut desired = enabled_configuration(vec![space]);
    handle.set_relay_configuration(desired.clone()).await?;
    let bound = handle
        .relay_status()
        .await?
        .private_listen_addr
        .ok_or("missing bound private listener")?;
    assert_ne!(bound.port(), 0);

    desired.listener_address = Some(bound.to_string());
    handle.set_relay_configuration(desired).await?;
    let status = handle.relay_status().await?;
    assert!(!status.convergence_pending);
    assert_eq!(status.private_listen_addr, Some(bound));
    runtime.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn wildcard_and_loopback_same_port_reconfiguration_stops_old_listener_first() -> TestResult {
    use super::tls_retry::{native_tls_config, write_tls_fixture};

    let state = TempState::new("wildcard-loopback")?;
    let transport = write_tls_fixture(&state)?;
    let runtime = Runtime::start(StoreConfig::new(&state.0)).await?;
    let handle = runtime.handle();
    let space = handle
        .create_owned_space("wildcard-loopback".to_owned())
        .await?;

    let wildcard_ephemeral: std::net::SocketAddr = ([0, 0, 0, 0], 0).into();
    handle
        .set_relay_configuration(native_tls_config(
            space,
            wildcard_ephemeral,
            transport.clone(),
        ))
        .await?;
    let wildcard = handle
        .relay_status()
        .await?
        .private_listen_addr
        .ok_or("missing bound wildcard private listener")?;
    assert_eq!(wildcard.ip(), std::net::IpAddr::from([0, 0, 0, 0]));
    assert_ne!(wildcard.port(), 0);
    let loopback: std::net::SocketAddr = ([127, 0, 0, 1], wildcard.port()).into();

    handle
        .set_relay_configuration(native_tls_config(space, loopback, transport.clone()))
        .await?;
    let tightened = handle.relay_status().await?;
    assert!(!tightened.convergence_pending);
    assert_eq!(tightened.private_listen_addr, Some(loopback));

    handle
        .set_relay_configuration(native_tls_config(space, wildcard, transport))
        .await?;
    let widened = handle.relay_status().await?;
    assert!(!widened.convergence_pending);
    assert_eq!(widened.private_listen_addr, Some(wildcard));
    runtime.shutdown().await?;
    Ok(())
}
