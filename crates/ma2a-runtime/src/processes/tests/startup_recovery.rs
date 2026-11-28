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

/// Persists an Agent whose previous unbound instance owns a `Created` Process, plus enough
/// extra `Created` rows to fill `MAX_ACTIVE_PROCESSES_PER_CREATOR` for `harness.local`.
async fn seed_unbound_agent_and_full_capacity(
    harness: &Harness,
    agent: &ma2a_core::service::AgentId,
) -> TestResult<ma2a_core::service::ProcessId> {
    let (prior_manager, prior_shutdown) = harness.manager();
    let agents = crate::agents::AgentManager::new(
        (
            harness.local,
            harness.store.clone(),
            std::sync::Arc::new(crate::clock::SystemClock),
        ),
        (&harness.state.0, prior_manager),
        prior_shutdown.clone(),
    );
    let spec = crate::agents::AgentSpec {
        id: agent.clone(),
        description: String::new(),
        command: ("/bin/true".to_owned(), Vec::new()),
        cwd: None,
        env: Vec::new(),
        tools: 0,
        max_tasks: 1,
    };
    let ((defined, _), _) = agents.define(spec).await?;
    let config = ma2a_store::StoreConfig::new(&harness.state.0);
    let mut repository = ma2a_store::Repository::open(&config)?;
    repository.set_agent_enabled(agent, (true, 2))?;
    let first = repository
        .begin_agent_instance(agent, 3)?
        .into_value()
        .and_then(|record| record.instance)
        .ok_or("no instance began")?;
    let mut hasher = blake3::Hasher::new_derive_key("ma2a agent instance start v1");
    hasher.update(agent.as_str().as_bytes());
    hasher.update(&[0]);
    hasher.update(&defined.times.0.to_be_bytes());
    hasher.update(&first.generation.to_be_bytes());
    let agent_req = ma2a_core::RequestId::try_from(&hasher.finalize().as_bytes()[..16])?;
    let raw_start = start("sleep 60")?.to_bytes()?;
    let agent_process = ma2a_core::service::ProcessId::random()?;
    repository.create_process(&ma2a_store::NewProcess {
        id: agent_process,
        binding: (harness.local, agent_req),
        fingerprint: [0x11; 32],
        start: (&raw_start, None),
        secret: [0x22; 32],
        created_ms: 4,
    })?;
    repository.end_agent_instance((agent, first.generation), 5)?;
    repository.set_agent_enabled(agent, (false, 6))?;
    for i in 1..ma2a_store::MAX_ACTIVE_PROCESSES_PER_CREATOR {
        let byte = u8::try_from(100 + i)?;
        repository.create_process(&ma2a_store::NewProcess {
            id: ma2a_core::service::ProcessId::random()?,
            binding: (harness.local, request_id(byte)?),
            fingerprint: [byte; 32],
            start: (&raw_start, None),
            secret: [byte; 32],
            created_ms: 4,
        })?;
    }
    prior_shutdown.cancel();
    Ok(agent_process)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn startup_recovery_retries_created_row_after_transient_store_error() -> TestResult {
    // Given: a persisted prior-boot `Created` row bound to an Agent's previous unbound instance,
    // plus seven more `Created` rows filling the local creator's active Process limit (8/8).
    let harness = Harness::new("c12-retry-created").await?;
    let agent = ma2a_core::service::AgentId::new("c12-agent")?;
    let agent_process = seed_unbound_agent_and_full_capacity(&harness, &agent).await?;
    let config = ma2a_store::StoreConfig::new(&harness.state.0);

    // Inject a transient Store write failure while startup recovery makes its first attempt.
    let sql = rusqlite::Connection::open(config.database_path())?;
    sql.execute_batch(
        "CREATE TRIGGER test_c12_created_fail BEFORE UPDATE ON processes
         BEGIN SELECT RAISE(ABORT, 'injected transient store failure'); END;",
    )?;

    let (manager, shutdown) = harness.manager();
    let recovered_agents = crate::agents::AgentManager::new(
        (
            harness.local,
            harness.store.clone(),
            std::sync::Arc::new(crate::clock::SystemClock),
        ),
        (&harness.state.0, manager.clone()),
        shutdown.clone(),
    );

    // When: startup recovery runs while the Store rejects process updates, then the Store recovers.
    manager.reconcile().await;
    tokio::time::sleep(Duration::from_millis(120)).await;
    let still_created = manager
        .record(agent_process)
        .await?
        .ok_or("missing agent process")?;
    assert_eq!(still_created.state, ProcessState::Created);
    sql.execute_batch("DROP TRIGGER test_c12_created_fail;")?;

    // Then: the same Runtime retries settling the Created rows to Failed(Interrupted), recovers
    // active capacity, and lets the Agent undefine cleanly.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        let record = manager
            .record(agent_process)
            .await?
            .ok_or("missing agent process")?;
        if record.state == ProcessState::Failed {
            assert_eq!(record.reason, Some(ProcessReason::Interrupted));
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "Created row never converged after transient Store error: {:?}",
            record.state
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    // Capacity is recovered: a new local start can now be recorded.
    let (creation, _) = manager
        .create_local((harness.local, request_id(199)?), &start("exit 0")?)
        .await?;
    assert!(
        matches!(creation.0, ProcessCreation::Created(_)),
        "expected recovered capacity, got {:?}",
        creation.0
    );
    // And the Agent can now proceed with undefine.
    recovered_agents.undefine(&agent).await?;
    shutdown.cancel();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reconcile_retries_initial_scan_on_transient_store_error_while_holding_fence() -> TestResult
{
    // Given: a prior-boot `Created` row and a fenced manager whose first Store command fails
    // transiently before subsequent commands succeed.
    let harness = Harness::new("c12-scan-retry").await?;
    let (prior_manager, prior_shutdown) = harness.manager();
    let prior =
        super::recovery::recorded(&harness, &prior_manager, (94, ProcessState::Created)).await?;
    prior_shutdown.cancel();

    let (store_tx, mut store_rx) = tokio::sync::mpsc::channel(crate::store::STORE_CAPACITY);
    let real_store = harness.store.sender.clone();
    tokio::spawn(async move {
        let mut failed_once = false;
        while let Some(command) = store_rx.recv().await {
            if !failed_once {
                failed_once = true;
                // Dropping the command makes StoreClient return Unavailable.
                drop(command);
                continue;
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

    // When: startup recovery runs and hits the one-time scan failure while a new start arrives.
    let reconcile_manager = manager.clone();
    let reconcile_task = tokio::spawn(async move { reconcile_manager.reconcile().await });
    let started = manager
        .start_local(request_id(95)?, start("exit 0")?)
        .await?;
    reconcile_task.await?;

    // Then: the prior-boot Created row was NOT abandoned (it converges to Failed(Interrupted)),
    // and the newly admitted start runs cleanly to Exited.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        let record = manager.record(prior).await?.ok_or("missing prior")?;
        if record.state == ProcessState::Failed {
            assert_eq!(record.reason, Some(ProcessReason::Interrupted));
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "prior-boot Created row was abandoned after transient scan error: {:?}",
            record.state
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    reaches(&manager, started.id, ProcessState::Exited).await?;
    shutdown.cancel();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn launch_failure_retries_created_to_spawn_failed_after_transient_store_error() -> TestResult
{
    // Given: a ProcessManager whose supervisor executable cannot launch, and a transient Store
    // write error that fails the immediate `Failed(SpawnFailed)` transition.
    let harness = Harness::new("l02-spawn-fail-retry").await?;
    let shutdown = tokio_util::sync::CancellationToken::new();
    let broken_hosting = super::ProcessHosting::executable(
        std::path::PathBuf::from("/nonexistent/ma2a-missing-supervisor"),
        Vec::new(),
    );
    let manager = crate::processes::ProcessManager::new(
        (
            harness.local,
            harness.store.clone(),
            std::sync::Arc::new(crate::clock::SystemClock),
        ),
        (&harness.state.0, Some(broken_hosting)),
        shutdown.clone(),
    );
    let config = ma2a_store::StoreConfig::new(&harness.state.0);
    let sql = rusqlite::Connection::open(config.database_path())?;
    sql.execute_batch(
        "CREATE TRIGGER test_l02_spawn_fail BEFORE UPDATE ON processes
         BEGIN SELECT RAISE(ABORT, 'injected transient store failure'); END;",
    )?;

    // When: `start_local` commits Created, fails launch, and fails to commit SpawnFailed.
    let req = request_id(96)?;
    let result = manager.start_local(req, start("exit 0")?).await;
    assert_eq!(
        result.err(),
        Some(ma2a_core::service::ServiceError::Unavailable)
    );
    let stuck = harness
        .store
        .processes(move |repo| repo.process_by_binding(harness.local, req))
        .await??
        .ok_or("missing Created row")?;
    assert_eq!(stuck.state, ProcessState::Created);

    // Then: once the Store recovers, the manager settles the Created row to Failed(SpawnFailed).
    sql.execute_batch("DROP TRIGGER test_l02_spawn_fail;")?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        let record = manager.record(stuck.id).await?.ok_or("missing row")?;
        if record.state == ProcessState::Failed {
            assert_eq!(record.reason, Some(ProcessReason::SpawnFailed));
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "launch-failed Created row stayed stuck after Store recovered: {:?}",
            record.state
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    shutdown.cancel();
    Ok(())
}
