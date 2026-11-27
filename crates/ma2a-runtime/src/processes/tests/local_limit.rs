//! C04: live-limit refusal must not archive an ended local Process or delete its log.

use ma2a_core::service::{ProcessReason, ProcessState, ServiceError};
use ma2a_store::MAX_ACTIVE_PROCESSES_PER_CREATOR;

use super::{Harness, TestResult, reaches, request_id, start};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn local_start_at_live_limit_preserves_ended_process_and_log() -> TestResult {
    // Given: one ended local Process with a readable start and log, plus 8 live local Processes.
    let harness = Harness::new("live-limit").await?;
    let (manager, shutdown) = harness.manager();
    let ended = manager
        .start_local(request_id(1)?, start("echo kept-output; exit 0")?)
        .await?;
    let ended = reaches(&manager, ended.id, ProcessState::Exited).await?;
    assert!(ended.start.is_some());
    assert!(ended.times.archived_ms.is_none());

    let live_limit = u8::try_from(MAX_ACTIVE_PROCESSES_PER_CREATOR)?;
    let mut live_ids = Vec::new();
    for offset in 0..live_limit {
        let record = manager
            .start_local(request_id(10 + offset)?, start("sleep 60")?)
            .await?;
        live_ids.push(record.id);
    }

    // When: another local start is refused at the live Process limit.
    let refused = manager
        .start_local(request_id(99)?, start("sleep 60")?)
        .await;

    // Then: the refusal does not archive the ended Process or delete its directory/log.
    assert_eq!(refused.err(), Some(ServiceError::ConcurrencyExceeded));
    let after = manager
        .local_record(ended.id)
        .await?
        .ok_or("ended Process missing")?;
    assert!(after.times.archived_ms.is_none(), "{after:?}");
    assert!(after.start.is_some(), "start bytes were cleared");
    assert!(std::fs::exists(manager.directory(ended.id))?);
    let log = manager
        .read_local_log(&after, (0, 64))
        .await?
        .ok_or("ended log missing")?;
    let bytes: Vec<u8> = log
        .records
        .into_iter()
        .flat_map(|(_, chunk)| chunk)
        .collect();
    assert_eq!(bytes, b"kept-output\n");

    for id in live_ids {
        let _ended = manager
            .terminate((None, id), (500, ProcessReason::Requested))
            .await;
    }
    shutdown.cancel();
    Ok(())
}
