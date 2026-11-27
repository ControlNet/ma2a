//! Recovery without a supervisor: only the status record decides, never a PID.

use ma2a_core::service::{
    FrameKind, LogFrom, ProcessId, ProcessReason, ProcessReply, ProcessState, ServiceError,
    Termination,
};
use ma2a_services::{
    FrameReader, FrameWriter,
    supervisor::{EndCause, SupervisorStatus, status},
};
use ma2a_store::{ProcessCreation, ProcessTransition};

use super::{Harness, TestResult, reaches, request_id, start, started};
use crate::processes::ProcessManager;

/// Records a Process and walks it to `state` without ever launching a supervisor.
pub(super) async fn recorded(
    harness: &Harness,
    manager: &ProcessManager,
    (byte, state): (u8, ProcessState),
) -> TestResult<ProcessId> {
    let binding = (harness.creator, request_id(byte)?);
    let (creation, _) = manager.create(binding, &start("sleep 60")?).await?;
    let ProcessCreation::Created(record) = creation else {
        return Err("expected a new Process".into());
    };
    let id = record.id;
    let path = match state {
        ProcessState::Created => &[][..],
        ProcessState::Starting => &[ProcessState::Starting][..],
        _ => &[ProcessState::Starting, ProcessState::Running][..],
    };
    for to in path.iter().copied() {
        let transition = ProcessTransition {
            to,
            reason: None,
            termination: None,
            at_ms: 1,
        };
        harness
            .store
            .processes(move |repository| repository.transition_process(id, transition))
            .await??;
    }
    Ok(id)
}

fn write_status(manager: &ProcessManager, id: ProcessId, next: SupervisorStatus) -> TestResult {
    let directory = manager.directory(id);
    std::fs::create_dir_all(&directory)?;
    status::write(&directory, next)?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_running_process_without_supervisor_or_recorded_end_is_lost() -> TestResult {
    let harness = Harness::new("lost").await?;
    let (manager, shutdown) = harness.manager();
    let id = recorded(&harness, &manager, (10, ProcessState::Running)).await?;
    // A status record that says running proves nothing once nobody answers.
    write_status(&manager, id, SupervisorStatus::Running(1, 1))?;
    manager.reconcile().await;
    let lost = reaches(&manager, id, ProcessState::Lost).await?;
    assert_eq!(lost.reason, Some(ProcessReason::SupervisorLost));
    shutdown.cancel();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_recorded_end_decides_a_process_whose_supervisor_is_gone() -> TestResult {
    let harness = Harness::new("recorded-end").await?;
    let (manager, shutdown) = harness.manager();
    let exited = recorded(&harness, &manager, (11, ProcessState::Running)).await?;
    let ended = SupervisorStatus::Ended(Termination::Exited(0), EndCause::Exited, 5);
    write_status(&manager, exited, ended)?;
    let starting = recorded(&harness, &manager, (12, ProcessState::Starting)).await?;
    write_status(&manager, starting, SupervisorStatus::NeverStarted(5))?;
    let created = recorded(&harness, &manager, (13, ProcessState::Created)).await?;
    manager.reconcile().await;

    let record = reaches(&manager, exited, ProcessState::Exited).await?;
    assert_eq!(record.termination, Some(Termination::Exited(0)));
    assert_eq!(record.times.ended_ms, Some(5));
    for id in [starting, created] {
        let record = reaches(&manager, id, ProcessState::Failed).await?;
        assert_eq!(record.reason, Some(ProcessReason::Interrupted));
    }
    shutdown.cancel();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delete_archives_ended_processes_and_keeps_their_binding() -> TestResult {
    let harness = Harness::new("delete").await?;
    let (manager, shutdown) = harness.manager();
    let active = started(&harness, &manager, (14, "sleep 60")).await?;
    assert_eq!(
        manager.delete(None, active.id).await,
        Err(ServiceError::ProcessActive)
    );
    let done = started(&harness, &manager, (15, "echo done")).await?;
    reaches(&manager, done.id, ProcessState::Exited).await?;
    // Another creator cannot see, let alone delete, this Process.
    let stranger = Some(iroh::SecretKey::generate().public().into());
    assert_eq!(
        manager.delete(stranger, done.id).await,
        Err(ServiceError::UnknownProcess)
    );
    let deleted = manager.delete(Some(harness.creator), done.id).await?;
    // The answer is the archive as committed, at the revision it committed.
    assert_eq!(deleted.value().id, done.id);
    assert!(deleted.value().is_archived());
    assert_eq!(
        deleted.revision(),
        manager.record_at(done.id).await?.revision()
    );
    assert!(!std::fs::exists(manager.directory(done.id))?);
    let binding = (harness.creator, request_id(15)?);
    let (retry, _) = manager.create(binding, &start("echo done")?).await?;
    let ProcessCreation::Existing(tombstone) = retry else {
        return Err("a retried start must answer the archived Process".into());
    };
    assert_eq!(tombstone.id, done.id);
    assert!(tombstone.is_archived());
    let target = (None, active.id);
    let terminated = manager
        .terminate(target, (500, ProcessReason::Requested))
        .await?;
    // The answer is the end as committed, at the revision it committed.
    assert_eq!(terminated.value().state, ProcessState::Terminated);
    let now = manager.record_at(active.id).await?;
    assert_eq!(terminated.revision(), now.revision());
    assert_eq!(Some(terminated.value()), now.value().as_ref());
    shutdown.cancel();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn logs_stay_readable_after_the_process_ended() -> TestResult {
    let harness = Harness::new("logs").await?;
    let (manager, shutdown) = harness.manager();
    let record = started(&harness, &manager, (16, "printf abc; printf de >&2")).await?;
    reaches(&manager, record.id, ProcessState::Exited).await?;
    let (client, server) = tokio::io::duplex(1 << 16);
    let mut writer = FrameWriter::new(server);
    let viewer = Some(harness.creator);
    let report = manager
        .logs(
            ((viewer, &shutdown), (record.id, LogFrom::Cursor(1), 1_024)),
            &mut writer,
        )
        .await;
    assert_eq!(report.bytes_out, 4);
    drop(writer);
    let mut reader = FrameReader::new(client);
    let mut output = Vec::new();
    let mut outcome = None;
    while let Some(frame) = reader.next().await? {
        match frame.kind() {
            FrameKind::Data => output.extend_from_slice(frame.data()?.1),
            FrameKind::Outcome => outcome = Some(ProcessReply::decode_outcome(frame.payload())?),
            _ => {}
        }
    }
    assert_eq!(output.len(), 4);
    assert_eq!(outcome, Some(Ok(ProcessReply::Logs(5, true))));
    shutdown.cancel();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_termination_reason_outlives_a_store_failure_of_the_end() -> TestResult {
    // Given: a Process being ended because its creator lost authorization.
    let harness = Harness::new("reason-kept").await?;
    let (manager, shutdown) = harness.manager();
    let id = recorded(&harness, &manager, (17, ProcessState::Running)).await?;
    manager.coordination().terminating.insert(
        id,
        crate::processes::Terminating::new(ProcessReason::Unauthorized),
    );
    let ended = SupervisorStatus::Ended(Termination::Signaled(15), EndCause::Terminated, 5);
    let config = ma2a_store::StoreConfig::new(&harness.state.0);
    let sql = rusqlite::Connection::open(config.database_path())?;
    sql.execute_batch(
        "CREATE TRIGGER test_process_end_failure BEFORE UPDATE ON processes
         BEGIN SELECT RAISE(ABORT, 'injected process end failure'); END;",
    )?;

    // When: committing the reported end fails once, and the next report commits it.
    let failed = manager.apply(id, ended).await;
    sql.execute_batch("DROP TRIGGER test_process_end_failure;")?;
    let committed = manager.apply(id, ended).await?;

    // Then: the end carries the reason it was asked for, not a requested one.
    assert_eq!(failed.err(), Some(ServiceError::Unavailable));
    assert_eq!(committed.value().state, ProcessState::Terminated);
    assert_eq!(committed.value().reason, Some(ProcessReason::Unauthorized));
    assert!(!manager.coordination().terminating.contains_key(&id));
    shutdown.cancel();
    Ok(())
}
