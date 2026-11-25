//! The caller side of a Process attachment against a scripted target.
//!
//! The target here is a test script writing exact Services v1 frames, not a Process host.

#[path = "support/host.rs"]
mod support;

use std::{
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};

use ma2a_core::{
    RequestId,
    service::{
        Channel, EndReason, FrameKind, LogFrom, LogWindow, ProcessAccept, ProcessId, ProcessReply,
        ProcessState, ProcessSummary, encode_skipped,
    },
};
use ma2a_services::client::{ClientError, hosted, process::ProcessIo};
use support::{TestResult, pipe};
use tokio::io::{AsyncRead, ReadBuf};

async fn within<T>(future: impl std::future::Future<Output = T>) -> TestResult<T> {
    Ok(tokio::time::timeout(Duration::from_secs(30), future).await?)
}

const fn process(byte: u8) -> ProcessId {
    ProcessId::from_bytes([byte; 16])
}

fn summary(id: ProcessId) -> TestResult<ProcessSummary> {
    Ok(ProcessSummary::new(
        id,
        RequestId::try_from([7_u8; 16].as_slice())?,
        0,
    ))
}

/// One frame the target sends: its kind and payload.
type Sent = (FrameKind, Vec<u8>);

fn reading(start: u64, window: (u64, u64)) -> TestResult<Sent> {
    let accept = ProcessAccept::Reading {
        state: ProcessState::Running,
        start,
        window: LogWindow::new(window.0, window.1),
        truncated: false,
    };
    Ok((FrameKind::Accept, accept.encode()?))
}

/// Output on stdout: channel 1, then the bytes.
fn data(bytes: &[u8]) -> Sent {
    (
        FrameKind::Data,
        [&[Channel::Output as u8][..], bytes].concat(),
    )
}

fn skipped(cursor: u64) -> TestResult<Sent> {
    Ok((FrameKind::Control, encode_skipped(cursor)?))
}

fn detached(reason: EndReason, id: ProcessId, cursor: u64) -> TestResult<Sent> {
    let reply = ProcessReply::Detached(reason, summary(id)?, cursor);
    Ok((
        FrameKind::Outcome,
        ProcessReply::encode_outcome(&Ok(reply))?,
    ))
}

/// Attaches to Process 1 from `from`, with stdin when `input` is given, against a target that
/// sends `frames`; returns the answer and what reached stdout.
async fn attach(
    from: LogFrom,
    input: Option<&[u8]>,
    frames: Vec<Sent>,
) -> TestResult<(Result<hosted::Attached, ClientError>, Vec<u8>)> {
    let ((mut reader, mut writer), (_service_reader, mut service_writer)) = pipe();
    for (kind, payload) in frames {
        service_writer.frame(kind, &payload).await?;
    }
    service_writer.flush().await?;
    let mut output = Vec::new();
    let local = ProcessIo {
        input,
        output: &mut output,
        error: None::<Vec<u8>>,
    };
    let target = (process(1), from);
    let attached = within(hosted::attach(
        (&mut reader, &mut writer),
        (target, local),
        std::future::pending(),
    ))
    .await?;
    Ok((attached, output))
}

#[tokio::test]
async fn an_attachment_is_where_its_bytes_and_skips_put_it() -> TestResult {
    let frames = vec![
        reading(100, (0, 1000))?,
        data(b"abc"),
        skipped(200)?,
        data(b"de"),
        detached(EndReason::Completed, process(1), 202)?,
    ];
    let (attached, output) = attach(LogFrom::Cursor(100), None, frames).await?;
    assert_eq!(attached?.cursor, 202);
    assert_eq!(output, b"abcde");
    Ok(())
}

#[tokio::test]
async fn an_attachment_answer_that_does_not_follow_its_request_is_refused() -> TestResult {
    let admitted = (FrameKind::Accept, ProcessAccept::Admitted.encode()?);
    for (case, frames) in [
        // A reader reserves nothing, so it is never answered from replay.
        (
            "no accept",
            vec![detached(EndReason::Completed, process(1), 100)?],
        ),
        ("not a read", vec![admitted]),
        // Before the cursor asked for: that output was printed already.
        ("rewound", vec![reading(50, (50, 1000))?, data(b"dup")]),
        (
            "skip back",
            vec![reading(100, (0, 1000))?, data(b"abc"), skipped(101)?],
        ),
        (
            "other cursor",
            vec![
                reading(100, (0, 1000))?,
                data(b"abc"),
                detached(EndReason::Completed, process(1), 999)?,
            ],
        ),
        (
            "other Process",
            vec![
                reading(100, (0, 1000))?,
                detached(EndReason::Completed, process(2), 100)?,
            ],
        ),
    ] {
        let (attached, output) = attach(LogFrom::Cursor(100), None, frames).await?;
        assert!(
            matches!(
                attached,
                Err(ClientError::Stream(
                    ma2a_core::service::ServiceError::InvalidInput
                ))
            ),
            "{case}: {attached:?}"
        );
        if case == "rewound" {
            assert!(output.is_empty(), "{case}: nothing printed");
        }
    }
    Ok(())
}

#[tokio::test]
async fn a_writing_attachment_may_be_replayed_but_only_for_its_own_process() -> TestResult {
    let replayed = vec![detached(EndReason::Completed, process(1), 7)?];
    let (attached, _) = attach(LogFrom::Now, Some(b""), replayed).await?;
    assert_eq!(attached?.cursor, 7);

    let other = vec![detached(EndReason::Completed, process(2), 7)?];
    let (attached, _) = attach(LogFrom::Now, Some(b""), other).await?;
    assert!(
        matches!(attached, Err(ClientError::Stream(_))),
        "{attached:?}"
    );
    Ok(())
}

/// Local input that delivers one chunk and then fails.
struct FailsAfter(Option<&'static [u8]>);

impl AsyncRead for FailsAfter {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Poll::Ready(self.0.take().map_or_else(
            || Err(std::io::ErrorKind::BrokenPipe.into()),
            |chunk| {
                buffer.put_slice(chunk);
                Ok(())
            },
        ))
    }
}

/// Attaches with stdin read from `input`, against a target that takes two frames from the
/// caller and then reports the attachment ended with `reason`; returns the answer and the
/// kinds of those two frames.
async fn attach_writing<I: AsyncRead + Unpin>(
    input: I,
    reason: EndReason,
) -> TestResult<(Result<hosted::Attached, ClientError>, Vec<FrameKind>)> {
    let ((mut reader, mut writer), (mut service_reader, mut service_writer)) = pipe();
    let target = async {
        let (kind, accept) = reading(0, (0, 0))?;
        service_writer.frame(kind, &accept).await?;
        service_writer.flush().await?;
        let first = service_reader.expect().await?;
        assert_eq!(first.data()?, (Channel::Input, &b"ab"[..]));
        let second = service_reader.expect().await?;
        let (kind, outcome) = detached(reason, process(1), 0)?;
        service_writer.frame(kind, &outcome).await?;
        service_writer.flush().await?;
        TestResult::Ok(vec![first.kind(), second.kind()])
    };
    let local = ProcessIo {
        input: Some(input),
        output: Vec::new(),
        error: None::<Vec<u8>>,
    };
    let caller = hosted::attach(
        (&mut reader, &mut writer),
        ((process(1), LogFrom::Now), local),
        std::future::pending(),
    );
    let (attached, kinds) = within(async { tokio::join!(caller, target) }).await?;
    Ok((attached, kinds?))
}

#[tokio::test]
async fn a_failed_local_read_detaches_without_closing_the_process_stdin() -> TestResult {
    // Given: local input that fails after one chunk.
    // When
    let (attached, kinds) = attach_writing(FailsAfter(Some(b"ab")), EndReason::Cancelled).await?;

    // Then: the attachment detaches instead of ending stdin, and the failure is reported.
    assert_eq!(kinds, [FrameKind::Data, FrameKind::Cancel]);
    assert!(
        matches!(attached, Err(ClientError::Local(_))),
        "{attached:?}"
    );
    Ok(())
}

#[tokio::test]
async fn the_end_of_local_input_closes_the_process_stdin() -> TestResult {
    let (attached, kinds) = attach_writing(&b"ab"[..], EndReason::Completed).await?;
    assert_eq!(kinds, [FrameKind::Data, FrameKind::End]);
    assert_eq!(attached?.reason, EndReason::Completed);
    Ok(())
}
