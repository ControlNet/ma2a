use std::sync::Arc;

use ma2a_store::{Repository, StoreConfig, StoreError};

use super::{
    maintenance::FaultPoint,
    relay_lifecycle_test::{TempState, disabled_configuration, enabled_configuration},
};
use crate::{
    Runtime,
    current_user::CurrentUserRuntime,
    error::RuntimeError,
    ipc::{IpcPaths, LocalApiServer, ServerExit},
    web::{SystemClock, WebAuthConfig},
};

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

async fn wait_for_candidate_attempts(handle: &super::RuntimeHandle, count: usize) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while handle.candidate_refresh_attempts() < count {
        assert!(
            std::time::Instant::now() < deadline,
            "periodic relay candidate reconciliation did not run"
        );
        tokio::task::yield_now().await;
    }
}

#[tokio::test(start_paused = true)]
async fn temporary_background_store_failure_keeps_runtime_alive_for_next_tick() -> TestResult {
    let state = TempState::new("transient-background")?;
    let config = StoreConfig::new(&state.0);
    let runtime = Runtime::start(config.clone()).await?;
    let handle = runtime.handle();
    let mut desired = disabled_configuration();
    desired.public_fallback_enabled = true;
    desired.public_relay_urls = vec!["https://retry.example.invalid".to_owned()];
    Repository::open(&config)?.set_relay_configuration(&desired)?;
    let before = handle.candidate_refresh_attempts();
    handle.fail_background_once(
        FaultPoint::RelayMapApply,
        RuntimeError::from(StoreError::Io(std::io::ErrorKind::Interrupted.into())),
    );
    let period = crate::control_actor::control_period(handle.status().await?.endpoint_id());

    tokio::time::advance(period).await;
    wait_for_candidate_attempts(&handle, before + 1).await;
    assert!(handle.status().await?.is_ready());
    assert_eq!(
        handle
            .snapshot()
            .await?
            .to_value()
            .pointer("/public_relay_fallbacks"),
        Some(&serde_json::json!([]))
    );
    tokio::time::advance(period).await;
    wait_for_candidate_attempts(&handle, before + 2).await;
    assert!(handle.status().await?.is_ready());
    let snapshot = handle.snapshot().await?.to_value();
    assert_eq!(
        snapshot.pointer("/public_relay_fallbacks/0/relay_url"),
        Some(&serde_json::json!("https://retry.example.invalid/"))
    );
    runtime.shutdown().await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn integrity_background_failure_stops_actor_and_local_server() -> TestResult {
    let state = TempState::new("fatal-background")?;
    let runtime = Runtime::start(StoreConfig::new(&state.0)).await?;
    let handle = runtime.handle();
    let control = CurrentUserRuntime::open_at(
        &state.0,
        Arc::new(SystemClock::default()),
        WebAuthConfig::default(),
    )
    .await?;
    let paths = IpcPaths::new(&state.0)?;
    let server = LocalApiServer::bind_for_launch(paths, handle.clone(), control, None).await?;
    let server_task = tokio::spawn(server.serve(tokio_util::sync::CancellationToken::new()));
    handle.fail_background_once(
        FaultPoint::RelayCandidateLoad,
        RuntimeError::from(StoreError::SchemaMismatch {
            detail: "injected invalid relay state",
        }),
    );
    let period = crate::control_actor::control_period(handle.status().await?.endpoint_id());
    tokio::time::advance(period).await;
    handle.stopped().await;
    assert_eq!(server_task.await??, ServerExit::RuntimeStopped);
    drop(runtime);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn committed_relay_publication_retry_keeps_its_sequence() -> TestResult {
    let state = TempState::new("relay-followup")?;
    let config = StoreConfig::new(&state.0);
    let runtime = Runtime::start(config.clone()).await?;
    let handle = runtime.handle();
    let space_id = handle
        .create_owned_space("relay-followup".to_owned())
        .await?;
    let endpoint_id = handle.status().await?.endpoint_id();
    handle.fail_background_once(
        FaultPoint::RelayPublication,
        RuntimeError::from(StoreError::Io(std::io::ErrorKind::Interrupted.into())),
    );
    assert!(
        handle
            .set_relay_configuration(enabled_configuration(vec![space_id]))
            .await
            .is_err()
    );
    let sequence = Repository::open(&config)?
        .relay_advertisement(space_id, endpoint_id)?
        .ok_or("relay advertisement did not commit before follow-up failure")?
        .sequence();
    let before_projection_revision = handle.status().await?.revision();
    let before = handle
        .control_schedules()
        .iter()
        .filter(|trigger| {
            matches!(
                trigger,
                crate::control_sync::ControlRoundTrigger::RelayAdvanced
            )
        })
        .count();
    tokio::time::advance(crate::control_actor::control_period(endpoint_id)).await;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while handle
        .control_schedules()
        .iter()
        .filter(|trigger| {
            matches!(
                trigger,
                crate::control_sync::ControlRoundTrigger::RelayAdvanced
            )
        })
        .count()
        <= before
    {
        assert!(
            std::time::Instant::now() < deadline,
            "relay publication follow-up did not complete"
        );
        tokio::task::yield_now().await;
    }
    let converged = handle.status().await?;
    assert!(converged.is_ready());
    assert!(converged.revision() > before_projection_revision);
    assert_eq!(
        Repository::open(&config)?
            .relay_advertisement(space_id, endpoint_id)?
            .ok_or("relay advertisement missing after retry")?
            .sequence(),
        sequence
    );
    runtime.shutdown().await?;
    Ok(())
}
