//! `process logs --follow` of a Process that ended before it could attach.

use ma2a_core::service::{
    Channel, FrameKind, LogFrom, LogWindow, ProcessAccept, ProcessDetails, ProcessId, ProcessReply,
    ProcessState, ProcessSummary, ServiceError, Termination,
};

use crate::test_daemon::{Daemon, Script, TestResult};

const PROCESS: ProcessId = ProcessId::from_bytes([3; 16]);

/// A read of Process [`PROCESS`], whose whole output is `hello`, from `start` to its end;
/// it says whether the Process `ended` with it.
fn read(start: u64, ended: bool) -> TestResult<Script> {
    let accept = ProcessAccept::Reading {
        state: ProcessState::Running,
        start,
        window: LogWindow::new(0, 5),
        truncated: false,
    };
    let output = b"hello".get(usize::try_from(start)?..).ok_or("start")?;
    let read = ProcessReply::encode_outcome(&Ok(ProcessReply::Logs(5, ended)))?;
    let mut frames = vec![(FrameKind::Accept, accept.encode()?)];
    if !output.is_empty() {
        frames.push((
            FrameKind::Data,
            [&[Channel::Output as u8][..], output].concat(),
        ));
    }
    frames.push((FrameKind::Outcome, read));
    Ok(frames)
}

/// An inspection of Process [`PROCESS`], which ended with `termination`.
fn inspected(termination: Option<Termination>) -> TestResult<Script> {
    let summary = ProcessSummary::new(PROCESS, ma2a_core::RequestId::random()?, 1_000)
        .with_state(ProcessState::Exited, (None, termination));
    let details = ProcessDetails::new(summary, None, LogWindow::new(0, 5));
    let reply = ProcessReply::encode_outcome(&Ok(ProcessReply::Details(Box::new(details))))?;
    Ok(vec![
        (FrameKind::Accept, ProcessAccept::Admitted.encode()?),
        (FrameKind::Outcome, reply),
    ])
}

/// The exit status of `process logs --follow` through the streams `opened` in turn.
async fn followed(opened: Vec<Script>) -> TestResult<i32> {
    let daemon = Daemon::paging("logs", opened)?;
    let target = (&daemon.paths, daemon.endpoint.as_str());
    let logs = super::hosted_stream::logs_code(target, (PROCESS, LogFrom::Start), true);
    Ok(tokio::time::timeout(std::time::Duration::from_secs(30), logs).await??)
}

#[tokio::test]
async fn following_a_process_that_already_ended_exits_with_its_status() -> TestResult {
    // Given: a Process that ended with status 42, or without one, before the read.
    for (termination, code) in [(Some(Termination::Exited(42)), 42), (None, 255)] {
        // When
        let followed = followed(vec![read(0, true)?, inspected(termination)?]).await?;

        // Then: as when the attachment sees it end.
        assert_eq!(followed, code, "{termination:?}");
    }
    Ok(())
}

#[tokio::test]
async fn following_a_process_that_ends_before_the_attach_exits_with_its_status() -> TestResult {
    // Given: a Process that ends between the read and the attach.
    let refused = vec![(FrameKind::Reject, vec![ServiceError::NotRunning.code()])];
    let opened = vec![
        read(0, false)?,
        refused,
        read(5, true)?,
        inspected(Some(Termination::Exited(42)))?,
    ];

    // When
    let followed = followed(opened).await?;

    // Then
    assert_eq!(followed, 42);
    Ok(())
}
