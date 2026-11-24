//! Reading the retained output of a Process whose log cannot be read.

use ma2a_core::service::{ProcessState, ServiceError};
use ma2a_services::supervisor::log_directory;

use super::{Harness, TestResult, reaches, started};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unreadable_log_is_an_io_failure_not_an_empty_window() -> TestResult {
    // Given: an ended Process whose log directory became a regular file.
    let harness = Harness::new("unreadable-log").await?;
    let (manager, shutdown) = harness.manager();
    let record = started(&harness, &manager, (9, "echo hi")).await?;
    reaches(&manager, record.id, ProcessState::Exited).await?;
    let directory = log_directory(&manager.directory(record.id));
    std::fs::remove_dir_all(&directory)?;
    std::fs::write(&directory, b"not a log")?;

    // When
    let inspected = manager.details(None, record.id).await;

    // Then
    assert_eq!(inspected.err(), Some(ServiceError::Io));
    shutdown.cancel();
    Ok(())
}
