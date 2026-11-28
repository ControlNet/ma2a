//! Startup recovery fencing and `starting`-before-`go` enforcement (C11).

use std::time::Duration;

use ma2a_core::service::{ProcessReason, ProcessState};
use ma2a_services::supervisor::{SupervisorStatus, status};
use ma2a_store::ProcessCreation;

use super::{Harness, TestResult, reaches, request_id, start};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bring_up_does_not_send_go_when_startup_recovery_already_failed_row() -> TestResult {
    // Given: a start that committed `Created` and is held before supervisor bind while startup
    // recovery runs and marks the row `Failed(Interrupted)` (clearing its secret).
    let harness = Harness::new("c11-no-go").await?;
    let (manager, shutdown) = harness.manager();
    let marker = harness.state.0.join("child-ran");
    let script = format!("touch {}; sleep 1", marker.display());
    let start_spec = start(&script)?;
    let binding = (harness.creator, request_id(91)?);
    let (creation, credentials) = manager.create(binding, &start_spec).await?;
    let ProcessCreation::Created(created) = creation else {
        return Err("expected a new Process".into());
    };
    assert_eq!(created.state, ProcessState::Created);

    // Startup recovery runs while launch has not bound yet.
    manager.reconcile().await;
    let interrupted = reaches(&manager, created.id, ProcessState::Failed).await?;
    assert_eq!(interrupted.reason, Some(ProcessReason::Interrupted));
    assert!(interrupted.secret.is_none());

    // When: the held launch is released and `bring_up` runs.
    let result = manager.bring_up(credentials, start_spec).await?;

    // Then: the row stays terminal `Failed(Interrupted)`, the supervisor is aborted promptly, and
    // no unmanaged child is ever started.
    assert_eq!(result.state, ProcessState::Failed);
    assert_eq!(result.reason, Some(ProcessReason::Interrupted));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !marker.exists(),
        "child must never be spawned when the Process row is already Failed(Interrupted)"
    );
    let recorded = status::read(&manager.directory(created.id))?;
    assert!(
        matches!(recorded, Some(SupervisorStatus::NeverStarted(_))),
        "supervisor should be aborted with NeverStarted, got {recorded:?}"
    );
    shutdown.cancel();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn startup_fence_holds_new_starts_until_recovery_scans_prior_rows() -> TestResult {
    // Given: a prior-boot `Created` row and a fenced manager whose startup scan is paused before
    // reading SQLite.
    let harness = Harness::new("c11-fence").await?;
    let (prior_manager, prior_shutdown) = harness.manager();
    let prior =
        super::recovery::recorded(&harness, &prior_manager, (92, ProcessState::Created)).await?;
    prior_shutdown.cancel();

    let (store_tx, mut store_rx) = tokio::sync::mpsc::channel(crate::store::STORE_CAPACITY);
    let real_store = harness.store.sender.clone();
    let scan_entered = std::sync::Arc::new(tokio::sync::Notify::new());
    let release_scan = std::sync::Arc::new(tokio::sync::Notify::new());
    let entered = std::sync::Arc::clone(&scan_entered);
    let release = std::sync::Arc::clone(&release_scan);
    tokio::spawn(async move {
        let mut paused_once = false;
        while let Some(command) = store_rx.recv().await {
            if !paused_once {
                paused_once = true;
                entered.notify_one();
                release.notified().await;
            }
            if real_store.send(command).await.is_err() {
                break;
            }
        }
    });

    let shutdown = tokio_util::sync::CancellationToken::new();
    let manager = crate::processes::ProcessManager::new(
        (
            harness.local,
            crate::store::StoreClient::new(store_tx),
            std::sync::Arc::new(crate::clock::SystemClock),
        ),
        (&harness.state.0, Some(super::ProcessHosting::in_process())),
        shutdown.clone(),
    );
    manager.fence_startup();

    // When: startup recovery is running its initial scan and a new start arrives concurrently.
    let reconcile_manager = manager.clone();
    let reconcile_task = tokio::spawn(async move { reconcile_manager.reconcile().await });
    scan_entered.notified().await;

    let start_manager = manager.clone();
    let new_req = request_id(93)?;
    let new_start = start("echo ok; exit 0")?;
    let mut start_task =
        tokio::spawn(async move { start_manager.start_local(new_req, new_start).await });
    // The new start must stay fenced while the recovery scan is still in progress.
    assert!(
        tokio::time::timeout(Duration::from_millis(150), &mut start_task)
            .await
            .is_err(),
        "new start must wait until startup recovery finishes scanning prior-boot rows"
    );

    release_scan.notify_one();
    reconcile_task.await?;
    let started_record = tokio::time::timeout(Duration::from_secs(5), start_task).await???;

    // Then: the prior-boot row becomes Failed(Interrupted), while the newly admitted start runs
    // and exits as a managed Process.
    let prior_ended = reaches(&manager, prior, ProcessState::Failed).await?;
    assert_eq!(prior_ended.reason, Some(ProcessReason::Interrupted));
    let new_ended = reaches(&manager, started_record.id, ProcessState::Exited).await?;
    assert_eq!(
        new_ended.termination,
        Some(ma2a_core::service::Termination::Exited(0))
    );
    shutdown.cancel();
    Ok(())
}
