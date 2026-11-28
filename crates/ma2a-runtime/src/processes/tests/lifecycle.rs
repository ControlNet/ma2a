//! Core Process lifecycle, `RequestId` binding, readoption, withdrawal, and local retention.

use std::time::Duration;

use ma2a_core::service::ProcessState;
use ma2a_store::ProcessCreation;

use super::{Harness, TestResult, reaches, request_id, start, started};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_started_process_runs_and_its_exit_is_recorded() -> TestResult {
    let harness = Harness::new("exit").await?;
    let (manager, shutdown) = harness.manager();
    let record = started(&harness, &manager, (1, "echo hi; sleep 0.2; exit 4")).await?;
    assert!(
        matches!(record.state, ProcessState::Running | ProcessState::Exited),
        "{:?}",
        record.state
    );
    let ended = reaches(&manager, record.id, ProcessState::Exited).await?;
    assert_eq!(
        ended.termination,
        Some(ma2a_core::service::Termination::Exited(4))
    );
    assert!(ended.secret.is_none(), "the secret is gone once terminal");
    assert_eq!(manager.window(&record).await?.end, 3);
    shutdown.cancel();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_request_id_binds_exactly_one_process() -> TestResult {
    let harness = Harness::new("binding").await?;
    let (manager, shutdown) = harness.manager();
    let first = started(&harness, &manager, (2, "sleep 60")).await?;
    let binding = (harness.creator, request_id(2)?);
    let (again, _) = manager.create(binding, &start("sleep 60")?).await?;
    let ProcessCreation::Existing(existing) = again else {
        return Err("a retry must answer the existing Process".into());
    };
    assert_eq!(existing.id, first.id);
    let (different, _) = manager.create(binding, &start("sleep 61")?).await?;
    assert!(matches!(different, ProcessCreation::Conflict));
    let target = (None, first.id);
    let ended = manager
        .terminate(
            target,
            (1_000, ma2a_core::service::ProcessReason::Requested),
        )
        .await?;
    assert_eq!(ended.value().state, ProcessState::Terminated);
    assert_eq!(
        ended.value().reason,
        Some(ma2a_core::service::ProcessReason::Requested)
    );
    shutdown.cancel();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_new_daemon_readopts_a_running_process_through_the_proof() -> TestResult {
    let harness = Harness::new("readopt").await?;
    let (first, first_shutdown) = harness.manager();
    let record = started(&harness, &first, (3, "sleep 60")).await?;
    reaches(&first, record.id, ProcessState::Running).await?;
    // The daemon goes away; its supervisor and child keep running.
    first_shutdown.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;

    let (second, second_shutdown) = harness.manager();
    second.reconcile().await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let still = reaches(&second, record.id, ProcessState::Running).await?;
    assert_eq!(still.times.started_ms, record.times.started_ms);
    let ended = second
        .terminate(
            (None, record.id),
            (1_000, ma2a_core::service::ProcessReason::Requested),
        )
        .await?;
    assert_eq!(ended.value().state, ProcessState::Terminated);
    second_shutdown.cancel();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn withdrawn_authorization_terminates_the_creators_processes() -> TestResult {
    let harness = Harness::new("withdrawn").await?;
    let (manager, shutdown) = harness.manager();
    let record = started(&harness, &manager, (4, "sleep 60")).await?;
    reaches(&manager, record.id, ProcessState::Running).await?;
    harness.withdraw_creator().await?;
    manager.converge().await;
    let ended = reaches(&manager, record.id, ProcessState::Terminated).await?;
    assert_eq!(
        ended.reason,
        Some(ma2a_core::service::ProcessReason::Unauthorized)
    );
    shutdown.cancel();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn local_starts_make_room_by_archiving_the_oldest_ended_local_process() -> TestResult {
    let harness = Harness::new("local-room").await?;
    let (manager, shutdown) = harness.manager();
    let retained = u8::try_from(ma2a_store::MAX_RETAINED_PROCESSES_PER_CREATOR)?;
    let mut ended = Vec::new();
    for byte in 1..=retained {
        let record = manager
            .start_local(request_id(byte)?, start("exit 0")?)
            .await?;
        ended.push(reaches(&manager, record.id, ProcessState::Exited).await?.id);
    }

    // Every retained slot holds an ended instance, yet the next one still starts.
    let next = manager
        .start_local(request_id(retained + 1)?, start("exit 0")?)
        .await?;
    let (first, rest) = ended.split_first().ok_or("no Process")?;
    let oldest = manager.local_record(*first).await?.ok_or("no record")?;
    assert!(oldest.times.archived_ms.is_some(), "{oldest:?}");
    for kept in rest {
        let kept = manager.local_record(*kept).await?.ok_or("no record")?;
        assert!(kept.times.archived_ms.is_none(), "{kept:?}");
    }
    reaches(&manager, next.id, ProcessState::Exited).await?;
    shutdown.cancel();
    Ok(())
}
