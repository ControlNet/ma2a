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

async fn setup_space(
    name: &str,
) -> Result<
    (TempState, StoreConfig, Runtime, ma2a_core::SpaceId),
    Box<dyn std::error::Error + Send + Sync>,
> {
    let state = TempState::new(name)?;
    let config = StoreConfig::new(&state.0);
    let runtime = Runtime::start(config.clone()).await?;
    let space = runtime.handle().create_owned_space(name.to_owned()).await?;
    Ok((state, config, runtime, space))
}

fn relay_ad(
    config: &StoreConfig,
    space: ma2a_core::SpaceId,
    endpoint: ma2a_core::EndpointId,
) -> Result<ma2a_store::PersistedRelayAdvertisement, Box<dyn std::error::Error + Send + Sync>> {
    Ok(Repository::open(config)?
        .relay_advertisement(space, endpoint)?
        .ok_or("relay advertisement missing")?)
}

#[tokio::test(start_paused = true)]
async fn failed_same_port_replacement_deactivates_advertisements_immediately() -> TestResult {
    let (_state, config, runtime, space) = setup_space("same-port-deactivate").await?;
    let handle = runtime.handle();
    let endpoint = handle.status().await?.endpoint_id();
    let peer_id = ma2a_core::EndpointId::from(iroh::SecretKey::from_bytes(&[0x45; 32]).public());
    add_peer(&config, space, peer_id)?;

    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    drop(listener);
    let mut desired = enabled_configuration(vec![space]);
    desired.listener_address = Some(address.to_string());
    handle.set_relay_configuration(desired.clone()).await?;

    let initial_ad = relay_ad(&config, space, endpoint)?;
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
    assert!(status.convergence_pending && status.applied_private.is_none());
    assert_eq!(status.configuration, desired);
    assert!(
        !relay_ad(&config, space, endpoint)?.is_active(),
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
    let recovered = relay_ad(&config, space, endpoint)?;
    assert!(recovered.is_active() && recovered.sequence() > initial_ad.sequence());
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
    let (_state, config, runtime, space) = setup_space("diff-port-keep-active").await?;
    let handle = runtime.handle();
    let endpoint = handle.status().await?.endpoint_id();
    let peer_id = ma2a_core::EndpointId::from(iroh::SecretKey::from_bytes(&[0x46; 32]).public());
    add_peer(&config, space, peer_id)?;

    let mut desired = enabled_configuration(vec![space]);
    handle.set_relay_configuration(desired.clone()).await?;
    let initial_ad = relay_ad(&config, space, endpoint)?;
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

    assert!(handle.relay_status().await?.applied_private.is_some());
    assert!(relay_ad(&config, space, endpoint)?.is_active());
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
    let (_state, _config, runtime, space) = setup_space("ephemeral-to-fixed").await?;
    let handle = runtime.handle();

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

async fn assert_relay_state(
    handle: &crate::RuntimeHandle,
    (expected_listen, expected_applied): (std::net::SocketAddr, std::net::SocketAddr),
    pending: bool,
) -> TestResult {
    let status = handle.relay_status().await?;
    assert_eq!(status.convergence_pending, pending);
    assert_eq!(status.private_listen_addr, Some(expected_listen));
    assert_eq!(
        status.applied_private.map(|cfg| cfg.listen_addr()),
        Some(expected_applied)
    );
    Ok(())
}

type TlsRelaySetup = (
    TempState,
    Runtime,
    ma2a_core::SpaceId,
    ma2a_store::RelayTransportConfiguration,
    std::net::SocketAddr,
);

async fn setup_tls_relay(
    name: &str,
    listen: std::net::SocketAddr,
) -> Result<TlsRelaySetup, Box<dyn std::error::Error + Send + Sync>> {
    use super::tls_retry::{native_tls_config, write_tls_fixture};

    let (state, _config, runtime, space) = setup_space(name).await?;
    let transport = write_tls_fixture(&state)?;
    let handle = runtime.handle();
    handle
        .set_relay_configuration(native_tls_config(space, listen, transport.clone()))
        .await?;
    let bound = handle
        .relay_status()
        .await?
        .private_listen_addr
        .ok_or("missing bound private listener")?;
    assert_ne!(bound.port(), 0);
    Ok((state, runtime, space, transport, bound))
}

#[tokio::test]
async fn wildcard_and_loopback_same_port_reconfiguration_stops_old_listener_first() -> TestResult {
    use super::tls_retry::native_tls_config;

    let (_state, runtime, space, transport, wildcard) =
        setup_tls_relay("wildcard-loopback", ([0, 0, 0, 0], 0).into()).await?;
    assert_eq!(wildcard.ip(), std::net::IpAddr::from([0, 0, 0, 0]));
    let handle = runtime.handle();
    let loopback: std::net::SocketAddr = ([127, 0, 0, 1], wildcard.port()).into();

    handle
        .set_relay_configuration(native_tls_config(space, loopback, transport.clone()))
        .await?;
    assert_relay_state(&handle, (loopback, loopback), false).await?;

    handle
        .set_relay_configuration(native_tls_config(space, wildcard, transport))
        .await?;
    assert_relay_state(&handle, (wildcard, wildcard), false).await?;
    runtime.shutdown().await?;
    Ok(())
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn mapped_wildcard_to_loopback_same_port_reconfiguration_stops_old_listener_first()
-> TestResult {
    use super::tls_retry::native_tls_config;

    let (_state, runtime, space, transport, bound) =
        setup_tls_relay("mapped-wildcard-loopback", "[::ffff:0.0.0.0]:0".parse()?).await?;
    let handle = runtime.handle();
    let loopback: std::net::SocketAddr = ([127, 0, 0, 1], bound.port()).into();
    handle
        .set_relay_configuration(native_tls_config(space, loopback, transport))
        .await?;
    assert_relay_state(&handle, (loopback, loopback), false).await?;
    runtime.shutdown().await?;
    Ok(())
}

#[cfg(target_os = "linux")]
#[tokio::test(start_paused = true)]
async fn mapped_wildcard_to_occupied_ipv6_loopback_keeps_old_relay_online() -> TestResult {
    use super::tls_retry::native_tls_config;

    let mapped_wildcard: std::net::SocketAddr = "[::ffff:0.0.0.0]:0".parse()?;
    let (_state, runtime, space, transport, bound) =
        setup_tls_relay("mapped-wildcard-ipv6-occupied", mapped_wildcard).await?;
    let handle = runtime.handle();
    let ipv4_loopback = (std::net::Ipv4Addr::LOCALHOST, bound.port());
    drop(tokio::net::TcpStream::connect(ipv4_loopback).await?);

    let ipv6_loopback: std::net::SocketAddr = (std::net::Ipv6Addr::LOCALHOST, bound.port()).into();
    let occupied_v6 = std::net::TcpListener::bind(ipv6_loopback)?;
    assert!(
        handle
            .set_relay_configuration(native_tls_config(space, ipv6_loopback, transport))
            .await
            .is_err()
    );
    assert_relay_state(&handle, (bound, mapped_wildcard), true).await?;
    drop(tokio::net::TcpStream::connect(ipv4_loopback).await?);

    drop(occupied_v6);
    tick(&handle).await;
    assert_relay_state(&handle, (ipv6_loopback, ipv6_loopback), false).await?;
    runtime.shutdown().await?;
    Ok(())
}
