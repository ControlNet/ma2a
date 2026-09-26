use super::relay_lifecycle_test::TempState;
use crate::Runtime;
use ma2a_store::{Repository, StoreConfig};

mod failures;
type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fixed_listener_reconfiguration_does_not_bind_over_its_own_server() -> TestResult {
    let state = TempState::new("fixed-listener")?;
    let config = StoreConfig::new(&state.0);
    let runtime = Runtime::start(config.clone()).await?;
    let handle = runtime.handle();
    let space = handle.create_owned_space("Relay".to_owned()).await?;
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    drop(listener);
    let mut desired = super::relay_lifecycle_test::enabled_configuration(vec![space]);
    desired.listener_address = Some(address.to_string());
    handle.set_relay_configuration(desired.clone()).await?;
    handle.set_relay_configuration(desired.clone()).await?;
    handle.reconfigure_private_relay(desired.clone()).await?;
    desired.private_relay_url = Some("https://updated.example.invalid".to_owned());
    handle.set_relay_configuration(desired.clone()).await?;
    assert_eq!(
        handle.relay_status().await?.private_listen_addr,
        Some(address)
    );
    let status = handle.relay_status().await?;
    assert!(!status.convergence_pending);
    assert_eq!(
        status.configuration,
        Repository::open(&config)?.relay_configuration()?
    );
    runtime.shutdown().await?;
    let restarted = Runtime::start(config).await?;
    assert_eq!(
        restarted.handle().relay_status().await?.private_listen_addr,
        Some(address)
    );
    restarted
        .handle()
        .set_relay_configuration(super::relay_lifecycle_test::disabled_configuration())
        .await?;
    assert!(
        restarted
            .handle()
            .relay_status()
            .await?
            .applied_private
            .is_none()
    );
    assert!(tokio::net::TcpStream::connect(address).await.is_err());
    restarted.shutdown().await?;
    Ok(())
}

async fn tick(handle: &super::RuntimeHandle) {
    let before = handle
        .control_schedules()
        .iter()
        .filter(|trigger| matches!(trigger, crate::control_sync::ControlRoundTrigger::Periodic))
        .count();
    tokio::time::advance(std::time::Duration::from_secs(90)).await;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while handle
        .control_schedules()
        .iter()
        .filter(|trigger| matches!(trigger, crate::control_sync::ControlRoundTrigger::Periodic))
        .count()
        <= before
    {
        assert!(
            std::time::Instant::now() < deadline,
            "maintenance did not complete"
        );
        tokio::task::yield_now().await;
    }
}

#[tokio::test(start_paused = true)]
async fn occupied_listener_retains_desired_state_and_retries_without_republication_churn()
-> TestResult {
    let state = TempState::new("occupied-listener")?;
    let config = StoreConfig::new(&state.0);
    let runtime = Runtime::start(config.clone()).await?;
    let handle = runtime.handle();
    let space = handle
        .create_owned_space("pending-listener".to_owned())
        .await?;
    let occupied = std::net::TcpListener::bind("127.0.0.1:0")?;
    let address = occupied.local_addr()?;
    let mut desired = super::relay_lifecycle_test::enabled_configuration(vec![space]);
    desired.listener_address = Some(address.to_string());
    let error = handle
        .set_relay_configuration(desired.clone())
        .await
        .expect_err("occupied listener must fail");
    assert!(error.committed_revision().is_some());
    assert!(error.is_retryable_background());
    assert_eq!(Repository::open(&config)?.relay_configuration()?, desired);
    let pending = handle.relay_status().await?;
    assert!(pending.convergence_pending);
    assert!(pending.applied_private.is_none());
    let endpoint = handle.status().await?.endpoint_id();
    assert!(
        Repository::open(&config)?
            .relay_advertisement(space, endpoint)?
            .is_none()
    );
    drop(occupied);
    tick(&handle).await;
    assert!(handle.status().await?.is_ready());
    assert_eq!(
        handle.relay_status().await?.private_listen_addr,
        Some(address)
    );
    assert!(!handle.relay_status().await?.convergence_pending);
    let advertisement = Repository::open(&config)?
        .relay_advertisement(space, endpoint)?
        .ok_or("missing advertisement")?;
    let revision = Repository::open(&config)?.revision()?;
    for _ in 0..3 {
        tick(&handle).await;
    }
    assert_eq!(Repository::open(&config)?.revision()?, revision);
    assert_eq!(
        Repository::open(&config)?
            .relay_advertisement(space, endpoint)?
            .ok_or("missing advertisement")?
            .sequence(),
        advertisement.sequence()
    );
    runtime.shutdown().await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn restart_with_temporarily_occupied_private_listener_retains_pending_role() -> TestResult {
    let state = TempState::new("restart-pending-role")?;
    let config = StoreConfig::new(&state.0);
    let runtime = Runtime::start(config.clone()).await?;
    let handle = runtime.handle();
    let endpoint = handle.status().await?.endpoint_id();
    let space = handle
        .create_owned_space("restart-pending".to_owned())
        .await?;
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    drop(listener);
    let mut desired = super::relay_lifecycle_test::enabled_configuration(vec![space]);
    desired.listener_address = Some(address.to_string());
    handle.set_relay_configuration(desired).await?;
    runtime.shutdown().await?;
    let occupied = std::net::TcpListener::bind(address)?;
    let restarted = Runtime::start(config).await?;
    let handle = restarted.handle();
    assert_eq!(handle.status().await?.endpoint_id(), endpoint);
    assert!(handle.relay_status().await?.convergence_pending);
    assert!(handle.relay_status().await?.applied_private.is_none());
    drop(occupied);
    tick(&handle).await;
    assert_eq!(
        handle.relay_status().await?.private_listen_addr,
        Some(address)
    );
    assert!(!handle.relay_status().await?.convergence_pending);
    restarted.shutdown().await?;
    Ok(())
}
