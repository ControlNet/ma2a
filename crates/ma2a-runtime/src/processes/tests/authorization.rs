//! C05: remote `start` and `starting` recovery honor authorization withdrawal and unavailability.

use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use interprocess::local_socket::tokio::prelude::*;
use ma2a_core::service::{
    FrameKind, ProcessReason, ProcessReply, ProcessRequest, ProcessState, ServiceError,
};
use ma2a_services::{
    FrameReader, FrameWriter,
    client::{ClientError, hosted},
    supervisor::{
        Credentials, SupervisorRequest, SupervisorStatus,
        proof::{prove_supervisor, read_challenge},
        status,
    },
};
use ma2a_store::{ProcessCreation, ProcessTransition};
use tokio::sync::{Notify, mpsc};
use tokio_util::sync::CancellationToken;

use super::{Harness, TestResult, pipe, reaches, request_id, start};
use crate::{
    clock::SystemClock,
    processes::{ProcessHosting, ProcessManager, serve},
    store::{ServiceStoreCommand, StoreClient, StoreCommand},
};

impl ProcessManager {
    fn arm_gate(
        slot: &crate::processes::go::TestGate,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Sender<()>,
    ) {
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        *slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((entered_tx, release_rx));
        (entered_rx, release_tx)
    }

    pub(in crate::processes) fn arm_before_go_gate(
        &self,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Sender<()>,
    ) {
        Self::arm_gate(&self.0.gates.before_go)
    }

    pub(in crate::processes) fn arm_after_authorize_gate(
        &self,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Sender<()>,
    ) {
        Self::arm_gate(&self.0.gates.after_authorize)
    }

    pub(in crate::processes) fn arm_after_final_authorize_gate(
        &self,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Sender<()>,
    ) {
        Self::arm_gate(&self.0.gates.after_final_authorize)
    }
}

/// Wraps `store` with a forwarder that holds ONLY the first Store command (the startup scan)
/// until `release` is notified, forwarding every subsequent command immediately.
fn gated_scan_store(
    store: &StoreClient,
) -> (StoreClient, Arc<Notify>, Arc<Notify>, Arc<AtomicUsize>) {
    let (store_tx, mut store_rx) = mpsc::channel(crate::store::STORE_CAPACITY);
    let real_store = store.sender.clone();
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let other_count = Arc::new(AtomicUsize::new(0));
    let entered_tx = Arc::clone(&entered);
    let release_rx = Arc::clone(&release);
    let count = Arc::clone(&other_count);
    tokio::spawn(async move {
        let mut paused_once = false;
        while let Some(command) = store_rx.recv().await {
            if !paused_once {
                paused_once = true;
                let entered_tx = Arc::clone(&entered_tx);
                let release_rx = Arc::clone(&release_rx);
                let real_store = real_store.clone();
                tokio::spawn(async move {
                    entered_tx.notify_one();
                    release_rx.notified().await;
                    let _sent = real_store.send(command).await;
                });
                continue;
            }
            count.fetch_add(1, Ordering::AcqRel);
            if real_store.send(command).await.is_err() {
                break;
            }
        }
    });
    (StoreClient::new(store_tx), entered, release, other_count)
}

/// Wraps `store` so the next `fail_count` `ServiceStoreCommand::Authorize` calls drop silently
/// (producing `ServiceError::Unavailable`), while every other Store command proceeds normally.
fn failing_authorize_store(
    store: &StoreClient,
    fail_count: usize,
) -> (StoreClient, Arc<AtomicUsize>) {
    let (store_tx, mut store_rx) = mpsc::channel(crate::store::STORE_CAPACITY);
    let real_store = store.sender.clone();
    let remaining = Arc::new(AtomicUsize::new(fail_count));
    let seen = Arc::new(AtomicUsize::new(0));
    let seen_tx = Arc::clone(&seen);
    tokio::spawn(async move {
        while let Some(command) = store_rx.recv().await {
            if matches!(
                command,
                StoreCommand::Services(ServiceStoreCommand::Authorize { .. })
            ) {
                seen_tx.fetch_add(1, Ordering::AcqRel);
                if remaining
                    .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_sub(1))
                    .is_ok()
                {
                    drop(command);
                    continue;
                }
            }
            if real_store.send(command).await.is_err() {
                break;
            }
        }
    });
    (StoreClient::new(store_tx), seen)
}

struct GatedScan {
    manager: ProcessManager,
    shutdown: CancellationToken,
    release_scan: Arc<Notify>,
    reconcile_task: tokio::task::JoinHandle<()>,
}

async fn start_gated_scan(harness: &Harness) -> GatedScan {
    let (gated_store, scan_entered, release_scan, _) = gated_scan_store(&harness.store);
    let shutdown = CancellationToken::new();
    let manager = ProcessManager::new(
        (harness.local, gated_store, Arc::new(SystemClock)),
        (&harness.state.0, Some(ProcessHosting::in_process())),
        shutdown.clone(),
    );
    manager.fence_startup();
    let reconcile_task = tokio::spawn({
        let manager = manager.clone();
        async move { manager.reconcile().await }
    });
    scan_entered.notified().await;
    GatedScan {
        manager,
        shutdown,
        release_scan,
        reconcile_task,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remote_start_waiting_on_startup_scan_honors_cancel_token() -> TestResult {
    let harness = Harness::new("c05-scan-cancel").await?;
    let gated = start_gated_scan(&harness).await;

    let marker = harness.state.0.join("child-scan-cancel");
    let start_spec = start(&format!("touch {}; sleep 1", marker.display()))?;
    let req = request_id(110)?;
    let ((mut reader, _writer), (mut host_reader, mut host_writer)) = pipe();
    let stop = CancellationToken::new();
    let host = tokio::spawn({
        let manager = gated.manager.clone();
        let stop = stop.clone();
        let creator = harness.creator;
        async move {
            let scope = ((creator, req), ProcessRequest::Start(start_spec), &stop);
            serve(&manager, scope, (&mut host_reader, &mut host_writer)).await
        }
    });
    let caller = tokio::spawn(async move { hosted::call(&mut reader).await });

    tokio::time::sleep(Duration::from_millis(80)).await;
    harness.withdraw_creator().await?;
    stop.cancel();

    let answer = tokio::time::timeout(Duration::from_millis(500), caller)
        .await
        .map_err(|_| "remote Start ignored cancel token while waiting on startup scan")??;
    let _report = tokio::time::timeout(Duration::from_millis(500), host).await??;
    assert!(matches!(
        answer,
        Err(ClientError::Rejected(
            ServiceError::Unavailable | ServiceError::Unauthorized
        ))
    ));
    let row = harness
        .store
        .processes(move |repo| repo.process_by_binding(harness.creator, req))
        .await??;
    assert!(row.is_none(), "no Process row may be created: {row:?}");

    gated.release_scan.notify_one();
    gated.reconcile_task.await?;
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(!marker.exists(), "withdrawn start launched a child");
    gated.shutdown.cancel();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remote_start_waiting_on_startup_scan_rechecks_auth_before_create() -> TestResult {
    let harness = Harness::new("c05-scan-recheck").await?;
    let gated = start_gated_scan(&harness).await;

    let marker = harness.state.0.join("child-scan-recheck");
    let start_spec = start(&format!("touch {}; sleep 1", marker.display()))?;
    let req = request_id(111)?;
    let ((mut reader, _writer), (mut host_reader, mut host_writer)) = pipe();
    let stop = CancellationToken::new();
    let host = tokio::spawn({
        let manager = gated.manager.clone();
        let stop = stop.clone();
        let creator = harness.creator;
        async move {
            let scope = ((creator, req), ProcessRequest::Start(start_spec), &stop);
            serve(&manager, scope, (&mut host_reader, &mut host_writer)).await
        }
    });
    let caller = tokio::spawn(async move { hosted::call(&mut reader).await });

    // Withdraw grant before the periodic watcher cancels `stop`, then release the scan.
    tokio::time::sleep(Duration::from_millis(80)).await;
    harness.withdraw_creator().await?;
    gated.release_scan.notify_one();
    gated.reconcile_task.await?;

    let answer = tokio::time::timeout(Duration::from_secs(2), caller).await??;
    let _report = tokio::time::timeout(Duration::from_secs(2), host).await??;
    assert!(matches!(
        answer,
        Err(ClientError::Rejected(ServiceError::Unauthorized))
    ));
    let row = harness
        .store
        .processes(move |repo| repo.process_by_binding(harness.creator, req))
        .await??;
    assert!(row.is_none(), "no Process row may be created: {row:?}");
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(!marker.exists(), "withdrawn start launched a child");
    gated.shutdown.cancel();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn disconnected_authorized_remote_start_runs_after_startup_scan() -> TestResult {
    let harness = Harness::new("c05-scan-disconnect").await?;
    let gated = start_gated_scan(&harness).await;

    let marker = harness.state.0.join("child-scan-disconnect");
    let start_spec = start(&format!("touch {}; exit 0", marker.display()))?;
    let req = request_id(116)?;
    let (caller, (mut host_reader, mut host_writer)) = pipe();
    let stop = CancellationToken::new();
    let host = tokio::spawn({
        let manager = gated.manager.clone();
        let creator = harness.creator;
        async move {
            let scope = ((creator, req), ProcessRequest::Start(start_spec), &stop);
            serve(&manager, scope, (&mut host_reader, &mut host_writer)).await
        }
    });

    tokio::time::sleep(Duration::from_millis(80)).await;
    drop(caller);
    gated.release_scan.notify_one();
    gated.reconcile_task.await?;

    let _report = tokio::time::timeout(Duration::from_secs(5), host).await??;
    let row = harness
        .store
        .processes(move |repo| repo.process_by_binding(harness.creator, req))
        .await??
        .ok_or("disconnected authorized start should still create a Process")?;
    reaches(&gated.manager, row.id, ProcessState::Exited).await?;
    assert!(
        marker.exists(),
        "authorized start should run after caller disconnect"
    );
    gated.shutdown.cancel();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remote_start_withdrawn_before_go_aborts_without_starting_child() -> TestResult {
    let harness = Harness::new("c05-before-go").await?;
    let (manager, shutdown) = harness.manager();
    let (entered_rx, release_tx) = manager.arm_before_go_gate();

    let marker = harness.state.0.join("child-before-go");
    let start_spec = start(&format!("touch {}; sleep 1", marker.display()))?;
    let req = request_id(112)?;
    let ((mut reader, _writer), (mut host_reader, mut host_writer)) = pipe();
    let stop = CancellationToken::new();
    let host = tokio::spawn({
        let manager = manager.clone();
        let creator = harness.creator;
        async move {
            let scope = ((creator, req), ProcessRequest::Start(start_spec), &stop);
            serve(&manager, scope, (&mut host_reader, &mut host_writer)).await
        }
    });
    let caller = tokio::spawn(async move { hosted::call(&mut reader).await });

    // Wait until Starting is committed and supervisor is connected immediately before Go.
    tokio::time::timeout(Duration::from_secs(5), entered_rx).await??;
    harness.withdraw_creator().await?;
    let _released = release_tx.send(());

    let (_accept, reply) = tokio::time::timeout(Duration::from_secs(5), caller).await???;
    let _report = tokio::time::timeout(Duration::from_secs(5), host).await??;
    let ProcessReply::Summary(summary) = reply else {
        return Err(format!("expected Summary reply, got {reply:?}").into());
    };
    assert_eq!(summary.state, ProcessState::Failed);
    assert_eq!(summary.reason, Some(ProcessReason::Interrupted));
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(
        !marker.exists(),
        "child must never start after pre-Go withdrawal"
    );
    let recorded = status::read(&manager.directory(summary.id))?;
    assert!(
        matches!(recorded, Some(SupervisorStatus::NeverStarted(_))),
        "expected NeverStarted supervisor status, got {recorded:?}"
    );
    shutdown.cancel();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resumed_starting_row_with_withdrawn_grant_aborts_without_go_on_restart() -> TestResult {
    // Given: a prior boot left a Starting row whose supervisor is still awaiting Go.
    let harness = Harness::new("c05-restart-starting").await?;
    let (prior_manager, prior_shutdown) = harness.manager();
    let marker = harness.state.0.join("child-restart-starting");
    let start_spec = start(&format!("touch {}; sleep 1", marker.display()))?;
    let (creation, credentials) = prior_manager
        .create((harness.creator, request_id(113)?), &start_spec)
        .await?;
    let ProcessCreation::Created(created) = creation else {
        return Err("expected Created row".into());
    };
    let _proven = prior_manager.launch(credentials, start_spec).await?;
    harness
        .store
        .processes(move |repo| {
            repo.transition_process(
                created.id,
                ProcessTransition {
                    to: ProcessState::Starting,
                    reason: None,
                    termination: None,
                    at_ms: 10,
                },
            )
        })
        .await??;
    prior_shutdown.cancel();

    // When: grant is withdrawn and the new Runtime runs startup recovery (without converge).
    harness.withdraw_creator().await?;
    let (second, second_shutdown) = harness.manager();
    second.reconcile().await;

    // Then: the follower settles Starting to Failed(Interrupted) without ever sending Go.
    let failed = reaches(&second, created.id, ProcessState::Failed).await?;
    assert_eq!(failed.reason, Some(ProcessReason::Interrupted));
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(
        !marker.exists(),
        "resumed Starting row started child despite withdrawn grant"
    );
    let recorded = status::read(&second.directory(created.id))?;
    assert!(
        matches!(recorded, Some(SupervisorStatus::NeverStarted(_))),
        "expected NeverStarted supervisor status, got {recorded:?}"
    );
    second_shutdown.cancel();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resumed_starting_row_withdrawn_at_before_go_gate_aborts_without_child() -> TestResult {
    let harness = Harness::new("c05-restart-gate").await?;
    let (prior_manager, prior_shutdown) = harness.manager();
    let marker = harness.state.0.join("child-restart-gate");
    let start_spec = start(&format!("touch {}; sleep 1", marker.display()))?;
    let (creation, credentials) = prior_manager
        .create((harness.creator, request_id(114)?), &start_spec)
        .await?;
    let ProcessCreation::Created(created) = creation else {
        return Err("expected Created row".into());
    };
    let _proven = prior_manager.launch(credentials, start_spec).await?;
    harness
        .store
        .processes(move |repo| {
            repo.transition_process(
                created.id,
                ProcessTransition {
                    to: ProcessState::Starting,
                    reason: None,
                    termination: None,
                    at_ms: 10,
                },
            )
        })
        .await??;
    prior_shutdown.cancel();

    let (second, second_shutdown) = harness.manager();
    let (entered_rx, release_tx) = second.arm_before_go_gate();
    second.reconcile().await;

    tokio::time::timeout(Duration::from_secs(5), entered_rx).await??;
    harness.withdraw_creator().await?;
    let _released = release_tx.send(());

    let failed = reaches(&second, created.id, ProcessState::Failed).await?;
    assert_eq!(failed.reason, Some(ProcessReason::Interrupted));
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(
        !marker.exists(),
        "resumed Starting row started child after pre-Go withdrawal"
    );
    second_shutdown.cancel();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unavailable_auth_check_before_go_retries_without_sending_go() -> TestResult {
    // Given: Store `Authorize` fails twice transiently (Unavailable) before succeeding, while
    // authorization is withdrawn after the first failure so the eventual retry must NOT launch.
    let harness = Harness::new("c05-auth-unavail").await?;
    let (flaky_store, seen) = failing_authorize_store(&harness.store, 2);
    let shutdown = CancellationToken::new();
    let manager = ProcessManager::new(
        (harness.local, flaky_store, Arc::new(SystemClock)),
        (&harness.state.0, Some(ProcessHosting::in_process())),
        shutdown.clone(),
    );

    let marker = harness.state.0.join("child-unavail");
    let start_spec = start(&format!("touch {}; sleep 1", marker.display()))?;
    let (creation, credentials) = manager
        .create((harness.creator, request_id(115)?), &start_spec)
        .await?;
    let ProcessCreation::Created(created) = creation else {
        return Err("expected Created row".into());
    };
    let _record = manager.bring_up(credentials, start_spec).await?;

    // Wait until at least one Authorize check ran and failed transiently, and withdraw grant
    // before the follower's retry succeeds.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while seen.load(Ordering::Acquire) < 1 {
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        !marker.exists(),
        "Go was sent while authorization check was unavailable"
    );
    harness.withdraw_creator().await?;

    let failed = reaches(&manager, created.id, ProcessState::Failed).await?;
    assert_eq!(failed.reason, Some(ProcessReason::Interrupted));
    assert!(seen.load(Ordering::Acquire) >= 3);
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(!marker.exists(), "child ran after retry");
    shutdown.cancel();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remote_start_withdrawn_after_authorize_reply_before_go_aborts_without_child() -> TestResult
{
    let harness = Harness::new("c07-start-post-auth").await?;
    let (manager, shutdown) = harness.manager();
    let (entered_rx, release_tx) = manager.arm_after_authorize_gate();

    let marker = harness.state.0.join("child-c07-start");
    let start_spec = start(&format!("touch {}; sleep 1", marker.display()))?;
    let req = request_id(117)?;
    let ((mut reader, _writer), (mut host_reader, mut host_writer)) = pipe();
    let stop = CancellationToken::new();
    let host = tokio::spawn({
        let manager = manager.clone();
        let creator = harness.creator;
        async move {
            let scope = ((creator, req), ProcessRequest::Start(start_spec), &stop);
            serve(&manager, scope, (&mut host_reader, &mut host_writer)).await
        }
    });
    let caller = tokio::spawn(async move { hosted::call(&mut reader).await });

    // Pause right after the authorization reply and before Go dispatch, then withdraw.
    tokio::time::timeout(Duration::from_secs(5), entered_rx).await??;
    harness.withdraw_creator().await?;
    let _released = release_tx.send(());

    let (_accept, reply) = tokio::time::timeout(Duration::from_secs(5), caller).await???;
    let _report = tokio::time::timeout(Duration::from_secs(5), host).await??;
    let ProcessReply::Summary(summary) = reply else {
        return Err(format!("expected Summary reply, got {reply:?}").into());
    };
    assert_eq!(summary.state, ProcessState::Failed);
    assert_eq!(summary.reason, Some(ProcessReason::Interrupted));
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(
        !marker.exists(),
        "child must never start after post-authorize withdrawal"
    );
    let recorded = status::read(&manager.directory(summary.id))?;
    assert!(
        matches!(recorded, Some(SupervisorStatus::NeverStarted(_))),
        "expected NeverStarted supervisor status, got {recorded:?}"
    );
    shutdown.cancel();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resumed_starting_row_withdrawn_after_authorize_reply_before_go_aborts() -> TestResult {
    let harness = Harness::new("c07-restart-post-auth").await?;
    let (prior_manager, prior_shutdown) = harness.manager();
    let marker = harness.state.0.join("child-c07-restart");
    let start_spec = start(&format!("touch {}; sleep 1", marker.display()))?;
    let (creation, credentials) = prior_manager
        .create((harness.creator, request_id(118)?), &start_spec)
        .await?;
    let ProcessCreation::Created(created) = creation else {
        return Err("expected Created row".into());
    };
    let _proven = prior_manager.launch(credentials, start_spec).await?;
    harness
        .store
        .processes(move |repo| {
            repo.transition_process(
                created.id,
                ProcessTransition {
                    to: ProcessState::Starting,
                    reason: None,
                    termination: None,
                    at_ms: 10,
                },
            )
        })
        .await??;
    prior_shutdown.cancel();

    let (second, second_shutdown) = harness.manager();
    let (entered_rx, release_tx) = second.arm_after_authorize_gate();
    second.reconcile().await;

    tokio::time::timeout(Duration::from_secs(5), entered_rx).await??;
    harness.withdraw_creator().await?;
    let _released = release_tx.send(());

    let failed = reaches(&second, created.id, ProcessState::Failed).await?;
    assert_eq!(failed.reason, Some(ProcessReason::Interrupted));
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(
        !marker.exists(),
        "resumed Starting row started child after post-authorize withdrawal"
    );
    let recorded = status::read(&second.directory(created.id))?;
    assert!(
        matches!(recorded, Some(SupervisorStatus::NeverStarted(_))),
        "expected NeverStarted supervisor status, got {recorded:?}"
    );
    second_shutdown.cancel();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resumed_starting_row_still_authorized_starts_on_recovery() -> TestResult {
    let harness = Harness::new("c07-restart-auth-ok").await?;
    let (prior_manager, prior_shutdown) = harness.manager();
    let marker = harness.state.0.join("child-c07-restart-ok");
    let start_spec = start(&format!("touch {}; exit 0", marker.display()))?;
    let (creation, credentials) = prior_manager
        .create((harness.creator, request_id(119)?), &start_spec)
        .await?;
    let ProcessCreation::Created(created) = creation else {
        return Err("expected Created row".into());
    };
    let _proven = prior_manager.launch(credentials, start_spec).await?;
    harness
        .store
        .processes(move |repo| {
            repo.transition_process(
                created.id,
                ProcessTransition {
                    to: ProcessState::Starting,
                    reason: None,
                    termination: None,
                    at_ms: 10,
                },
            )
        })
        .await??;
    prior_shutdown.cancel();

    let (second, second_shutdown) = harness.manager();
    let (entered_rx, release_tx) = second.arm_after_authorize_gate();
    second.reconcile().await;

    // An unrelated Store revision bump while Go is pending must not abort a still-authorized start.
    tokio::time::timeout(Duration::from_secs(5), entered_rx).await??;
    harness.store.advance_revision().await?;
    let _released = release_tx.send(());

    reaches(&second, created.id, ProcessState::Exited).await?;
    assert!(
        marker.exists(),
        "still-authorized resumed Starting row should start and run child"
    );
    second_shutdown.cancel();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remote_start_withdrawn_after_go_hold_expires_aborts_without_child() -> TestResult {
    let harness = Harness::new("c01-start-hold-expired").await?;
    harness
        .store
        .set_go_dispatch_hold(Duration::from_millis(80))
        .await;
    let (manager, shutdown) = harness.manager();
    let (entered_rx, release_tx) = manager.arm_after_final_authorize_gate();

    let marker = harness.state.0.join("child-c01-start");
    let start_spec = start(&format!("touch {}; sleep 1", marker.display()))?;
    let req = request_id(120)?;
    let ((mut reader, _writer), (mut host_reader, mut host_writer)) = pipe();
    let stop = CancellationToken::new();
    let host = tokio::spawn({
        let manager = manager.clone();
        let creator = harness.creator;
        async move {
            let scope = ((creator, req), ProcessRequest::Start(start_spec), &stop);
            serve(&manager, scope, (&mut host_reader, &mut host_writer)).await
        }
    });
    let caller = tokio::spawn(async move { hosted::call(&mut reader).await });

    // Pause right after the final Store authorization reply while the Store holds the lease.
    tokio::time::timeout(Duration::from_secs(5), entered_rx).await??;
    // Withdrawing blocks on the Store until the hold expires, then commits before Go is sent.
    harness.withdraw_creator().await?;
    let _released = release_tx.send(());

    let (_accept, reply) = tokio::time::timeout(Duration::from_secs(5), caller).await???;
    let _report = tokio::time::timeout(Duration::from_secs(5), host).await??;
    let ProcessReply::Summary(summary) = reply else {
        return Err(format!("expected Summary reply, got {reply:?}").into());
    };
    assert_eq!(summary.state, ProcessState::Failed);
    assert_eq!(summary.reason, Some(ProcessReason::Interrupted));
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(
        !marker.exists(),
        "child must never start after Go hold expiry and withdrawal"
    );
    let recorded = status::read(&manager.directory(summary.id))?;
    assert!(
        matches!(recorded, Some(SupervisorStatus::NeverStarted(_))),
        "expected NeverStarted supervisor status, got {recorded:?}"
    );
    shutdown.cancel();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resumed_starting_row_withdrawn_after_go_hold_expires_aborts_without_child() -> TestResult {
    let harness = Harness::new("c01-restart-hold-expired").await?;
    let (prior_manager, prior_shutdown) = harness.manager();
    let marker = harness.state.0.join("child-c01-restart");
    let start_spec = start(&format!("touch {}; sleep 1", marker.display()))?;
    let (creation, credentials) = prior_manager
        .create((harness.creator, request_id(121)?), &start_spec)
        .await?;
    let ProcessCreation::Created(created) = creation else {
        return Err("expected Created row".into());
    };
    let _proven = prior_manager.launch(credentials, start_spec).await?;
    harness
        .store
        .processes(move |repo| {
            repo.transition_process(
                created.id,
                ProcessTransition {
                    to: ProcessState::Starting,
                    reason: None,
                    termination: None,
                    at_ms: 10,
                },
            )
        })
        .await??;
    prior_shutdown.cancel();

    harness
        .store
        .set_go_dispatch_hold(Duration::from_millis(80))
        .await;
    let (second, second_shutdown) = harness.manager();
    let (entered_rx, release_tx) = second.arm_after_final_authorize_gate();
    second.reconcile().await;

    tokio::time::timeout(Duration::from_secs(5), entered_rx).await??;
    harness.withdraw_creator().await?;
    let _released = release_tx.send(());

    let failed = reaches(&second, created.id, ProcessState::Failed).await?;
    assert_eq!(failed.reason, Some(ProcessReason::Interrupted));
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(
        !marker.exists(),
        "resumed Starting row started child after Go hold expiry and withdrawal"
    );
    let recorded = status::read(&second.directory(created.id))?;
    assert!(
        matches!(recorded, Some(SupervisorStatus::NeverStarted(_))),
        "expected NeverStarted supervisor status, got {recorded:?}"
    );
    second_shutdown.cancel();
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn go_write_stops_mid_frame_when_lease_expires() -> TestResult {
    use tokio::io::AsyncReadExt as _;

    use crate::processes::go::{WriteGoOutcome, go_frame, write_go_with_lease};

    let frame = go_frame()?;
    assert!(frame.len() > 2);
    let (mut reader, mut writer) = tokio::io::duplex(2);
    let lease = crate::store::GoDispatchLease::new();

    {
        let write_fut = write_go_with_lease(&mut writer, &frame, &lease);
        tokio::pin!(write_fut);
        let mut ready_outcome = None;
        tokio::select! {
            biased;
            outcome = &mut write_fut => {
                ready_outcome = Some(outcome);
            }
            () = tokio::task::yield_now() => {}
        }
        assert!(ready_outcome.is_none(), "write must block after 2 bytes");

        lease.wait_until(std::time::Instant::now());
        assert_eq!(write_fut.await, WriteGoOutcome::Expired { wrote_any: true });
    }
    drop(writer);

    let mut received = Vec::new();
    reader.read_to_end(&mut received).await?;
    assert_eq!(Some(received.as_slice()), frame.get(..2));
    Ok(())
}

async fn accept_proven_connection(
    listener: &interprocess::local_socket::tokio::Listener,
    credentials: &Credentials,
) -> TestResult<(
    FrameReader<interprocess::local_socket::tokio::RecvHalf>,
    FrameWriter<interprocess::local_socket::tokio::SendHalf>,
)> {
    let mut stream = listener.accept().await?;
    let challenge = read_challenge(&mut FrameReader::new(&mut stream)).await?;
    let (recv, send) = stream.split();
    let (mut reader, mut writer) = (FrameReader::new(recv), FrameWriter::new(send));
    prove_supervisor(challenge, (&mut reader, &mut writer), credentials).await?;
    Ok((reader, writer))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remote_start_lease_expiry_with_production_deadlines_and_unchanged_membership_aborts()
-> TestResult {
    let harness = Harness::new("c02-prod-expiry").await?;
    let (manager, shutdown) = harness.manager();
    let (entered_rx, release_tx) = manager.arm_after_final_authorize_gate();

    let marker = harness.state.0.join("child-c02-prod-expiry");
    let start_spec = start(&format!("touch {}; sleep 1", marker.display()))?;
    let req = request_id(122)?;
    let ((mut reader, _writer), (mut host_reader, mut host_writer)) = pipe();
    let stop = CancellationToken::new();
    let host = tokio::spawn({
        let manager = manager.clone();
        let creator = harness.creator;
        async move {
            let scope = ((creator, req), ProcessRequest::Start(start_spec), &stop);
            serve(&manager, scope, (&mut host_reader, &mut host_writer)).await
        }
    });
    let caller = tokio::spawn(async move { hosted::call(&mut reader).await });

    // Wait until Store authorization finished, then let both the 5 s supervisor connect
    // deadline and the 6 s production Store Go hold expire with unchanged membership.
    tokio::time::timeout(Duration::from_secs(5), entered_rx).await??;
    tokio::time::sleep(Duration::from_secs(6) + Duration::from_millis(200)).await;
    harness.store.advance_revision().await?;
    let _released = release_tx.send(());

    let (_accept, reply) = tokio::time::timeout(Duration::from_secs(10), caller).await???;
    let _report = tokio::time::timeout(Duration::from_secs(10), host).await??;
    let ProcessReply::Summary(summary) = reply else {
        return Err(format!("expected Summary reply, got {reply:?}").into());
    };
    assert_eq!(summary.state, ProcessState::Failed);
    assert_eq!(summary.reason, Some(ProcessReason::Interrupted));
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(
        !marker.exists(),
        "child started after Go dispatch lease expired"
    );
    let recorded = status::read(&manager.directory(summary.id))?;
    assert!(
        matches!(recorded, Some(SupervisorStatus::NeverStarted(_))),
        "expected NeverStarted supervisor status, got {recorded:?}"
    );
    shutdown.cancel();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lease_expiry_abort_decision_persists_across_transient_settlement_failure() -> TestResult {
    let harness = Harness::new("c02-abort-retry").await?;
    let (prior_manager, prior_shutdown) = harness.manager();
    let start_spec = start("sleep 1")?;
    let (creation, credentials) = prior_manager
        .create((harness.creator, request_id(123)?), &start_spec)
        .await?;
    let ProcessCreation::Created(created) = creation else {
        return Err("expected Created row".into());
    };
    harness
        .store
        .processes(move |repo| {
            repo.transition_process(
                created.id,
                ProcessTransition {
                    to: ProcessState::Starting,
                    reason: None,
                    termination: None,
                    at_ms: 10,
                },
            )
        })
        .await??;
    prior_shutdown.cancel();

    let (second, second_shutdown) = harness.manager();
    let address = second.address(created.id);
    if let Some(parent) = std::path::Path::new(address.as_str()).parent() {
        std::fs::create_dir_all(parent)?;
    }
    let listener = address.bind()?;
    let (initial_dropped_tx, initial_dropped_rx) = tokio::sync::oneshot::channel();
    let (abort_seen_tx, abort_seen_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        // 1. Initial connection for dispatch_go closes once the lease expires (simulating
        //    supervisor CONNECT_DEADLINE < GO_DISPATCH_HOLD).
        let initial = accept_proven_connection(&listener, &credentials).await?;
        let _closed = initial_dropped_rx.await;
        drop(initial);
        // 2. First Abort reconnect drops before proving itself (transient Unavailable).
        drop(listener.accept().await?);
        // 3. Follower retry MUST send Abort, never Go, even with unchanged membership.
        let (mut reader, mut writer) = accept_proven_connection(&listener, &credentials).await?;
        let req = reader.expect().await?;
        let parsed = SupervisorRequest::from_bytes(req.payload())?;
        let _sent = abort_seen_tx.send(parsed);
        writer
            .frame(
                FrameKind::Outcome,
                &SupervisorStatus::NeverStarted(10).to_bytes(),
            )
            .await?;
        writer.shutdown().await?;
        TestResult::Ok(())
    });

    harness
        .store
        .set_go_dispatch_hold(Duration::from_millis(80))
        .await;
    let (entered_rx, release_tx) = second.arm_after_final_authorize_gate();
    second.reconcile().await;

    tokio::time::timeout(Duration::from_secs(5), entered_rx).await??;
    tokio::time::sleep(Duration::from_millis(120)).await;
    harness.store.advance_revision().await?;
    let _close_initial = initial_dropped_tx.send(());
    let _released = release_tx.send(());

    let parsed = tokio::time::timeout(Duration::from_secs(5), abort_seen_rx).await??;
    assert_eq!(parsed, SupervisorRequest::Abort);
    server.await??;
    let failed = reaches(&second, created.id, ProcessState::Failed).await?;
    assert_eq!(failed.reason, Some(ProcessReason::Interrupted));
    second_shutdown.cancel();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resumed_starting_row_active_after_go_hold_expires_re_adopts_authorized_child() -> TestResult
{
    let harness = Harness::new("a04-active-expiry").await?;
    let (prior_manager, prior_shutdown) = harness.manager();
    let marker = harness.state.0.join("child-a04-active");
    let start_spec = start(&format!("touch {}; sleep 2", marker.display()))?;
    let (creation, credentials) = prior_manager
        .create((harness.creator, request_id(124)?), &start_spec)
        .await?;
    let ProcessCreation::Created(created) = creation else {
        return Err("expected Created row".into());
    };
    let _watch = prior_manager.launch(credentials, start_spec).await?;
    harness
        .store
        .processes(move |repo| {
            repo.transition_process(
                created.id,
                ProcessTransition {
                    to: ProcessState::Starting,
                    reason: None,
                    termination: None,
                    at_ms: 10,
                },
            )
        })
        .await??;
    let go_channel = prior_manager
        .address(created.id)
        .connect(&credentials)
        .await?;
    let reported = crate::processes::follow::request(go_channel, SupervisorRequest::Go).await?;
    assert!(
        matches!(reported, SupervisorStatus::Running(_, _)),
        "expected Running after prior Go, got {reported:?}"
    );
    prior_shutdown.cancel();

    harness
        .store
        .set_go_dispatch_hold(Duration::from_millis(80))
        .await;
    let (second, second_shutdown) = harness.manager();
    let (entered_rx, release_tx) = second.arm_after_final_authorize_gate();
    second.reconcile().await;

    tokio::time::timeout(Duration::from_secs(5), entered_rx).await??;
    tokio::time::sleep(Duration::from_millis(120)).await;
    harness.store.advance_revision().await?;
    let _released = release_tx.send(());

    let running = reaches(&second, created.id, ProcessState::Running).await?;
    assert_eq!(running.state, ProcessState::Running);
    assert!(marker.exists(), "authorized child must keep running");
    let _ended = second
        .terminate((None, created.id), (100, ProcessReason::Requested))
        .await;
    second_shutdown.cancel();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resumed_starting_row_active_after_go_hold_expires_and_withdrawal_terminates() -> TestResult
{
    let harness = Harness::new("a04-active-withdrawn").await?;
    let (prior_manager, prior_shutdown) = harness.manager();
    let marker = harness.state.0.join("child-a04-withdrawn");
    let start_spec = start(&format!("touch {}; sleep 5", marker.display()))?;
    let (creation, credentials) = prior_manager
        .create((harness.creator, request_id(126)?), &start_spec)
        .await?;
    let ProcessCreation::Created(created) = creation else {
        return Err("expected Created row".into());
    };
    let _watch = prior_manager.launch(credentials, start_spec).await?;
    harness
        .store
        .processes(move |repo| {
            repo.transition_process(
                created.id,
                ProcessTransition {
                    to: ProcessState::Starting,
                    reason: None,
                    termination: None,
                    at_ms: 10,
                },
            )
        })
        .await??;
    let go_channel = prior_manager
        .address(created.id)
        .connect(&credentials)
        .await?;
    let reported = crate::processes::follow::request(go_channel, SupervisorRequest::Go).await?;
    assert!(matches!(reported, SupervisorStatus::Running(_, _)));
    prior_shutdown.cancel();

    harness
        .store
        .set_go_dispatch_hold(Duration::from_millis(80))
        .await;
    let (second, second_shutdown) = harness.manager();
    let (entered_rx, release_tx) = second.arm_after_final_authorize_gate();
    second.reconcile().await;

    tokio::time::timeout(Duration::from_secs(5), entered_rx).await??;
    harness.withdraw_creator().await?;
    let _released = release_tx.send(());

    let ended = reaches(&second, created.id, ProcessState::Terminated).await?;
    assert_eq!(ended.reason, Some(ProcessReason::Unauthorized));
    second_shutdown.cancel();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remote_start_maps_abort_failed_unreachable_to_unavailable() -> TestResult {
    let harness = Harness::new("a05-abort-unavail").await?;
    harness
        .store
        .set_go_dispatch_hold(Duration::from_millis(80))
        .await;
    let (manager, shutdown) = harness.manager();
    let (entered_rx, release_tx) = manager.arm_after_final_authorize_gate();

    let marker = harness.state.0.join("child-a05-unavail");
    let start_spec = start(&format!("touch {}; sleep 1", marker.display()))?;
    let req = request_id(125)?;
    let ((mut reader, _writer), (mut host_reader, mut host_writer)) = pipe();
    let stop = CancellationToken::new();
    let host = tokio::spawn({
        let manager = manager.clone();
        let creator = harness.creator;
        async move {
            let scope = ((creator, req), ProcessRequest::Start(start_spec), &stop);
            serve(&manager, scope, (&mut host_reader, &mut host_writer)).await
        }
    });
    let caller = tokio::spawn(async move { hosted::call(&mut reader).await });

    tokio::time::timeout(Duration::from_secs(5), entered_rx).await??;
    tokio::time::sleep(Duration::from_millis(120)).await;
    harness.store.advance_revision().await?;
    let row = harness
        .store
        .processes(move |repo| repo.process_by_binding(harness.creator, req))
        .await??
        .ok_or("missing created Process")?;
    let _removed = std::fs::remove_file(manager.address(row.id).as_str());
    let _released = release_tx.send(());

    let answer = tokio::time::timeout(Duration::from_secs(5), caller).await??;
    let _report = tokio::time::timeout(Duration::from_secs(5), host).await??;
    assert!(
        matches!(answer, Err(ClientError::Failed(ServiceError::Unavailable))),
        "expected Failed(Unavailable), got {answer:?}"
    );
    assert!(!marker.exists(), "child must not run");
    shutdown.cancel();
    Ok(())
}

#[test]
fn start_abort_error_maps_supervisor_and_cancelled_errors_to_unavailable() {
    use crate::processes::start::start_abort_error;

    for raw in [
        ServiceError::Unreachable,
        ServiceError::Unauthorized,
        ServiceError::Cancelled,
        ServiceError::Unavailable,
    ] {
        assert_eq!(start_abort_error(raw), ServiceError::Unavailable);
    }
    assert_eq!(
        start_abort_error(ServiceError::UnknownProcess),
        ServiceError::UnknownProcess
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remote_start_lease_expiry_with_convergence_does_not_send_go() -> TestResult {
    let harness = Harness::new("a01-converge-race").await?;
    harness
        .store
        .set_go_dispatch_hold(Duration::from_millis(80))
        .await;
    let (manager, shutdown) = harness.manager();
    let (entered_rx, release_tx) = manager.arm_after_final_authorize_gate();

    let marker = harness.state.0.join("child-a01-converge-race");
    let start_spec = start(&format!("touch {}; sleep 1", marker.display()))?;
    let req = request_id(127)?;
    let ((mut reader, _writer), (mut host_reader, mut host_writer)) = pipe();
    let stop = CancellationToken::new();
    let host = tokio::spawn({
        let manager = manager.clone();
        let creator = harness.creator;
        async move {
            let scope = ((creator, req), ProcessRequest::Start(start_spec), &stop);
            serve(&manager, scope, (&mut host_reader, &mut host_writer)).await
        }
    });
    let caller = tokio::spawn(async move { hosted::call(&mut reader).await });

    tokio::time::timeout(Duration::from_secs(5), entered_rx).await??;
    let row = harness
        .store
        .processes(move |repo| repo.process_by_binding(harness.creator, req))
        .await??
        .ok_or("missing Starting Process")?;
    assert_eq!(row.state, ProcessState::Starting);

    // Run convergence both while bring-up still holds the lease and after the hold expires.
    manager.converge().await;
    tokio::time::sleep(Duration::from_millis(120)).await;
    harness.store.advance_revision().await?;
    manager.converge().await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(
        !marker.exists(),
        "convergence must not start the child while bring-up owns Starting"
    );
    assert_eq!(
        status::read(&manager.directory(row.id))?,
        None,
        "supervisor must not receive Go from convergence while bring-up owns Starting"
    );

    let _released = release_tx.send(());
    let (_accept, reply) = tokio::time::timeout(Duration::from_secs(10), caller).await???;
    let _report = tokio::time::timeout(Duration::from_secs(10), host).await??;
    let ProcessReply::Summary(summary) = reply else {
        return Err(format!("expected Summary reply, got {reply:?}").into());
    };
    assert_eq!(summary.state, ProcessState::Failed);
    assert_eq!(summary.reason, Some(ProcessReason::Interrupted));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!marker.exists(), "child must not start after abort");
    let recorded = status::read(&manager.directory(summary.id))?;
    assert!(
        matches!(recorded, Some(SupervisorStatus::NeverStarted(_))),
        "expected NeverStarted supervisor status, got {recorded:?}"
    );
    shutdown.cancel();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn aborted_starting_recorded_before_go_write_diverts_to_abort() -> TestResult {
    let harness = Harness::new("a01-recheck-abort").await?;
    let (prior_manager, prior_shutdown) = harness.manager();
    let start_spec = start("sleep 1")?;
    let (creation, credentials) = prior_manager
        .create((harness.creator, request_id(128)?), &start_spec)
        .await?;
    let ProcessCreation::Created(created) = creation else {
        return Err("expected Created row".into());
    };
    harness
        .store
        .processes(move |repo| {
            repo.transition_process(
                created.id,
                ProcessTransition {
                    to: ProcessState::Starting,
                    reason: None,
                    termination: None,
                    at_ms: 10,
                },
            )
        })
        .await??;
    prior_shutdown.cancel();

    let (manager, shutdown) = harness.manager();
    let address = manager.address(created.id);
    if let Some(parent) = std::path::Path::new(address.as_str()).parent() {
        std::fs::create_dir_all(parent)?;
    }
    let listener = address.bind()?;
    let (seen_tx, seen_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut reader, mut writer) = accept_proven_connection(&listener, &credentials).await?;
        let req = reader.expect().await?;
        let parsed = SupervisorRequest::from_bytes(req.payload())?;
        let _sent = seen_tx.send(parsed);
        let reply = match parsed {
            SupervisorRequest::Abort => SupervisorStatus::NeverStarted(10),
            _ => SupervisorStatus::Running(1, 10),
        };
        writer.frame(FrameKind::Outcome, &reply.to_bytes()).await?;
        writer.shutdown().await?;
        TestResult::Ok(())
    });

    let (entered_rx, release_tx) = manager.arm_after_final_authorize_gate();
    manager.follow(created.id, None);
    tokio::time::timeout(Duration::from_secs(5), entered_rx).await??;
    manager.record_aborted_starting(created.id, crate::processes::go::AbortCause::Expired);
    let _released = release_tx.send(());

    let parsed = tokio::time::timeout(Duration::from_secs(5), seen_rx).await??;
    assert_eq!(parsed, SupervisorRequest::Abort);
    server.await??;
    let failed = reaches(&manager, created.id, ProcessState::Failed).await?;
    assert_eq!(failed.reason, Some(ProcessReason::Interrupted));
    shutdown.cancel();
    Ok(())
}
