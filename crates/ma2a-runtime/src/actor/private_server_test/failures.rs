use super::{TempState, TestResult, tick};
use crate::{
    Runtime,
    actor::{maintenance::FaultPoint, relay_lifecycle_test::enabled_configuration},
    error::{RuntimeError, RuntimeErrorKind},
};
use ma2a_store::{Repository, StoreConfig, StoreError};

#[tokio::test]
async fn invalid_tls_reconfiguration_preserves_desired_and_applied_server() -> TestResult {
    let state = TempState::new("invalid-tls-reconfigure")?;
    let config = StoreConfig::new(&state.0);
    let runtime = Runtime::start(config.clone()).await?;
    let handle = runtime.handle();
    let space = handle.create_owned_space("tls-input".to_owned()).await?;
    let desired = enabled_configuration(vec![space]);
    handle.set_relay_configuration(desired.clone()).await?;
    let before = handle.relay_status().await?;
    let revision = Repository::open(&config)?.revision()?;
    let mut invalid = desired.clone();
    invalid.transport = Some(ma2a_store::RelayTransportConfiguration::NativeTls {
        certificate_path: state.0.join("missing.cert.pem").display().to_string(),
        private_key_path: state.0.join("missing.key.pem").display().to_string(),
    });
    assert!(handle.reconfigure_private_relay(invalid).await.is_err());
    let after = handle.relay_status().await?;
    assert_eq!(before.private_listen_addr, after.private_listen_addr);
    assert_eq!(before.applied_private, after.applied_private);
    assert!(!after.convergence_pending);
    let repository = Repository::open(&config)?;
    assert_eq!(repository.relay_configuration()?, desired);
    assert_eq!(repository.revision()?, revision);
    runtime.shutdown().await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn failed_same_port_replacement_recovers_after_old_listener_release() -> TestResult {
    let state = TempState::new("replacement-start-failure")?;
    let config = StoreConfig::new(&state.0);
    let runtime = Runtime::start(config.clone()).await?;
    let handle = runtime.handle();
    let space = handle.create_owned_space("replace".to_owned()).await?;
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    drop(listener);
    let mut desired = enabled_configuration(vec![space]);
    desired.listener_address = Some(address.to_string());
    handle.set_relay_configuration(desired.clone()).await?;
    desired.private_relay_url = Some("https://replacement.example.invalid".to_owned());
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
    tick(&handle).await;
    assert_eq!(
        handle.relay_status().await?.private_listen_addr,
        Some(address)
    );
    assert!(!handle.relay_status().await?.convergence_pending);
    runtime.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn configuration_commit_failure_preserves_the_applied_listener() -> TestResult {
    let state = TempState::new("relay-config-commit-failure")?;
    let config = StoreConfig::new(&state.0);
    let runtime = Runtime::start(config.clone()).await?;
    let handle = runtime.handle();
    let space = handle
        .create_owned_space("store-failure".to_owned())
        .await?;
    let desired = enabled_configuration(vec![space]);
    handle.set_relay_configuration(desired.clone()).await?;
    let before = handle.relay_status().await?;
    let sql = rusqlite::Connection::open(config.database_path())?;
    // Test-only failure at the actual durable configuration update boundary.
    sql.execute_batch("CREATE TRIGGER test_relay_commit_failure BEFORE UPDATE ON relay_configuration BEGIN SELECT RAISE(ABORT, 'injected relay commit failure'); END;")?;
    let mut changed = desired.clone();
    changed.private_relay_url = Some("https://uncommitted.example.invalid".to_owned());
    assert!(handle.set_relay_configuration(changed).await.is_err());
    let after = handle.relay_status().await?;
    assert_eq!(before.private_listen_addr, after.private_listen_addr);
    assert_eq!(before.applied_private, after.applied_private);
    assert!(!after.convergence_pending);
    assert_eq!(Repository::open(&config)?.relay_configuration()?, desired);
    sql.execute_batch("DROP TRIGGER test_relay_commit_failure;")?;
    runtime.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn private_server_shutdown_failure_terminates_runtime() -> TestResult {
    let state = TempState::new("relay-shutdown-failure")?;
    let runtime = Runtime::start(StoreConfig::new(&state.0)).await?;
    let handle = runtime.handle();
    let space = handle
        .create_owned_space("shutdown-failure".to_owned())
        .await?;
    let desired = enabled_configuration(vec![space]);
    handle.set_relay_configuration(desired.clone()).await?;
    // Inject after the real server has joined, so the test leaves no listener behind.
    handle.fail_background_once(
        FaultPoint::PrivateRelayShutdown,
        RuntimeError::new(RuntimeErrorKind::Shutdown),
    );
    assert!(handle.reconfigure_private_relay(desired).await.is_err());
    handle.stopped().await;
    assert!(runtime.shutdown().await.is_err());
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn different_listener_failure_keeps_old_server_until_replacement_is_ready() -> TestResult {
    let state = TempState::new("different-listener")?;
    let config = StoreConfig::new(&state.0);
    let runtime = Runtime::start(config).await?;
    let handle = runtime.handle();
    let space = handle
        .create_owned_space("different-listener".to_owned())
        .await?;
    let mut desired = enabled_configuration(vec![space]);
    handle.set_relay_configuration(desired.clone()).await?;
    let old = handle
        .relay_status()
        .await?
        .private_listen_addr
        .ok_or("no old listener")?;
    let occupied = std::net::TcpListener::bind("127.0.0.1:0")?;
    let new = occupied.local_addr()?;
    desired.listener_address = Some(new.to_string());
    assert!(handle.set_relay_configuration(desired).await.is_err());
    let pending = handle.relay_status().await?;
    assert!(pending.convergence_pending);
    assert_eq!(pending.private_listen_addr, Some(old));
    assert!(pending.applied_private.is_some());
    drop(occupied);
    tick(&handle).await;
    assert_eq!(handle.relay_status().await?.private_listen_addr, Some(new));
    assert!(tokio::net::TcpStream::connect(old).await.is_err());
    runtime.shutdown().await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn publication_failure_does_not_restart_an_already_applied_server() -> TestResult {
    let state = TempState::new("server-publication-followup")?;
    let config = StoreConfig::new(&state.0);
    let runtime = Runtime::start(config).await?;
    let handle = runtime.handle();
    let space = handle
        .create_owned_space("publication-followup".to_owned())
        .await?;
    handle.fail_background_once(
        FaultPoint::RelayPublication,
        RuntimeError::from(StoreError::Io(std::io::ErrorKind::Interrupted.into())),
    );
    assert!(
        handle
            .set_relay_configuration(enabled_configuration(vec![space]))
            .await
            .is_err()
    );
    let installed = handle.relay_status().await?;
    assert!(installed.convergence_pending);
    assert!(installed.applied_private.is_some());
    // A restart on the next tick would hit this fault and fail convergence.
    handle.fail_background_once(
        FaultPoint::PrivateRelayStart,
        RuntimeError::from(StoreError::Io(std::io::ErrorKind::Interrupted.into())),
    );
    tick(&handle).await;
    let completed = handle.relay_status().await?;
    assert!(!completed.convergence_pending);
    assert_eq!(completed.private_listen_addr, installed.private_listen_addr);
    runtime.shutdown().await?;
    Ok(())
}
