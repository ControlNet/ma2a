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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deleted_output_is_never_served_even_when_its_files_stay() -> TestResult {
    use std::os::unix::fs::PermissionsExt as _;

    use ma2a_core::service::{FrameKind, LogFrom, LogWindow, ProcessReply};
    use ma2a_services::{FrameReader, FrameWriter};

    // Given: an ended Process with output, whose log cannot be removed but stays readable.
    let harness = Harness::new("deleted-log").await?;
    let (manager, shutdown) = harness.manager();
    let record = started(&harness, &manager, (10, "printf abc")).await?;
    reaches(&manager, record.id, ProcessState::Exited).await?;
    let directory = log_directory(&manager.directory(record.id));
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o500))?;

    // When: its creator deletes it, then reads it.
    let creator = Some(harness.creator);
    let deleted = manager.delete(creator, record.id).await;
    let inspected = manager.details(creator, record.id).await;
    let (client, server) = tokio::io::duplex(1 << 16);
    let mut writer = FrameWriter::new(server);
    let request = (record.id, LogFrom::Start, 1_024);
    let report = manager
        .logs(((creator, &shutdown), request), &mut writer)
        .await;
    drop(writer);
    let mut frames = Vec::new();
    let mut reader = FrameReader::new(client);
    while let Some(frame) = reader.next().await? {
        frames.push(frame);
    }
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;

    // Then: the deletion holds although the files stayed; no byte of them is served.
    assert!(deleted.is_ok(), "{deleted:?}");
    assert!(std::fs::exists(&directory)?, "the files stayed");
    let (_, details) = inspected?.into_value();
    assert_eq!(details.log, LogWindow::default());
    assert_eq!(report.bytes_out, 0);
    let kinds: Vec<FrameKind> = frames.iter().map(ma2a_services::Frame::kind).collect();
    assert_eq!(kinds, [FrameKind::Accept, FrameKind::Outcome]);
    let outcome = frames.last().ok_or("no outcome")?;
    assert_eq!(
        ProcessReply::decode_outcome(outcome.payload())?,
        Ok(ProcessReply::Logs(0, true))
    );
    shutdown.cancel();
    Ok(())
}
