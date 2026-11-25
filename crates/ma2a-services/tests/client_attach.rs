//! The caller side of Process attachments and stdin writes against a scripted target.
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
        ProcessState, ProcessSummary, ServiceError, encode_skipped,
    },
};
use ma2a_services::client::{ClientError, hosted, process::ProcessIo};
use support::{Flushes, TestResult, pipe};
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

/// The outcome of an attachment that ended with `reason`: a completed one saw the Process
/// exit, a detached one left it running.
fn detached(reason: EndReason, id: ProcessId, cursor: u64) -> TestResult<Sent> {
    let mut summary = summary(id)?;
    if reason == EndReason::Completed {
        summary.state = ProcessState::Exited;
        summary.termination = Some(ma2a_core::service::Termination::Exited(0));
    } else {
        summary.state = ProcessState::Running;
    }
    detached_as(reason, summary, cursor)
}

fn detached_as(reason: EndReason, summary: ProcessSummary, cursor: u64) -> TestResult<Sent> {
    let reply = ProcessReply::Detached(reason, summary, cursor);
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
        reading(100, (0, 150))?,
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
        // Completed while output the accept reported as retained was never sent.
        (
            "completed early",
            vec![
                reading(100, (0, 1000))?,
                data(b"abc"),
                detached(EndReason::Completed, process(1), 103)?,
            ],
        ),
        // Completed while the Process still runs.
        (
            "completed alive",
            vec![
                reading(100, (0, 100))?,
                detached_as(EndReason::Completed, summary(process(1))?, 100)?,
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
async fn a_detach_may_come_anywhere_while_the_process_runs() -> TestResult {
    let frames = vec![
        reading(100, (0, 1000))?,
        data(b"abc"),
        detached(EndReason::Cancelled, process(1), 103)?,
    ];
    let (attached, output) = attach(LogFrom::Cursor(100), None, frames).await?;
    assert_eq!(attached?.cursor, 103);
    assert_eq!(output, b"abc");
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
    attach_writing_through(input, std::convert::identity, reason).await
}

/// As [`attach_writing`], with the caller's stream wrapped by `wrap`.
async fn attach_writing_through<I, W>(
    input: I,
    wrap: impl FnOnce(tokio::io::WriteHalf<tokio::io::DuplexStream>) -> W,
    reason: EndReason,
) -> TestResult<(Result<hosted::Attached, ClientError>, Vec<FrameKind>)>
where
    I: AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let ((mut reader, writer), (mut service_reader, mut service_writer)) = pipe();
    let mut writer = ma2a_services::FrameWriter::new(wrap(writer.into_inner()));
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

#[tokio::test(start_paused = true)]
async fn the_end_of_local_input_reaches_the_target_through_a_buffered_stream() -> TestResult {
    // Given: a caller stream that holds writes until flushed.
    // When: local input ends after one chunk.
    let (attached, kinds) =
        attach_writing_through(&b"ab"[..], tokio::io::BufWriter::new, EndReason::Completed).await?;

    // Then: the target receives the `end` without anything else being sent after it.
    assert_eq!(kinds, [FrameKind::Data, FrameKind::End]);
    assert_eq!(attached?.reason, EndReason::Completed);
    Ok(())
}

/// A pipe that fails on every write.
struct Broken;

impl tokio::io::AsyncWrite for Broken {
    fn poll_write(
        self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        _buffer: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()))
    }

    fn poll_flush(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()))
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        _context: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

/// Attaches with stdout going to a pipe that fails, against a target that sends `abc`, takes
/// the caller's next frame, sends `after`, and then reports a detach at `cursor`; returns the
/// answer and the kind of the caller's frame.
async fn attach_failing_output(
    after: Vec<Sent>,
    cursor: u64,
) -> TestResult<(Result<hosted::Attached, ClientError>, FrameKind)> {
    let ((mut reader, mut writer), (mut service_reader, mut service_writer)) = pipe();
    let target = async {
        for (kind, payload) in [reading(0, (0, 3))?, data(b"abc")] {
            service_writer.frame(kind, &payload).await?;
        }
        service_writer.flush().await?;
        let seen = service_reader.expect().await?.kind();
        for (kind, payload) in after {
            service_writer.frame(kind, &payload).await?;
        }
        let (kind, outcome) = detached(EndReason::Cancelled, process(1), cursor)?;
        service_writer.frame(kind, &outcome).await?;
        service_writer.flush().await?;
        TestResult::Ok(seen)
    };
    let local = ProcessIo {
        input: None::<&[u8]>,
        output: Broken,
        error: None::<Vec<u8>>,
    };
    let caller = hosted::attach(
        (&mut reader, &mut writer),
        ((process(1), LogFrom::Start), local),
        std::future::pending(),
    );
    let (attached, seen) = within(async { tokio::join!(caller, target) }).await?;
    Ok((attached, seen?))
}

#[tokio::test]
async fn a_failed_local_output_write_detaches_and_is_reported() -> TestResult {
    // Given: a local output that fails.
    // When: the attachment detaches at the cursor its bytes reached.
    let (attached, seen) = attach_failing_output(Vec::new(), 3).await?;

    // Then: the target is told to detach, and the failed write is the answer.
    assert_eq!(seen, FrameKind::Cancel);
    assert!(
        matches!(attached, Err(ClientError::Local(_))),
        "{attached:?}"
    );
    Ok(())
}

#[tokio::test]
async fn input_sent_back_by_the_target_is_refused_after_a_failed_output_write() -> TestResult {
    // Given: a local output that fails, and a target that then sends channel 0 to the caller.
    let input = (
        FrameKind::Data,
        [&[Channel::Input as u8][..], b"zz"].concat(),
    );

    // When
    let (attached, _) = attach_failing_output(vec![input], 5).await?;

    // Then
    assert!(
        matches!(
            attached,
            Err(ClientError::Stream(ServiceError::InvalidInput))
        ),
        "{attached:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_stdin_write_that_could_not_be_sent_still_reports_why_the_target_ended() -> TestResult {
    // Given: a target that admits the write and, a moment after the caller's stream broke,
    // reports that the Process stdin was closed.
    let ((mut reader, _writer), (_service_reader, mut service_writer)) = pipe();
    let target = async {
        let admitted = ProcessAccept::Admitted.encode()?;
        service_writer.frame(FrameKind::Accept, &admitted).await?;
        service_writer.flush().await?;
        tokio::time::sleep(Duration::from_millis(50)).await;
        let closed = ProcessReply::encode_outcome(&Err(ServiceError::StdinClosed))?;
        service_writer.frame(FrameKind::Outcome, &closed).await?;
        service_writer.flush().await?;
        TestResult::Ok(())
    };

    // When
    let mut broken = ma2a_services::FrameWriter::new(Broken);
    let caller = hosted::stdin((&mut reader, &mut broken), (&b"abc"[..], true));
    let (written, sent) = within(async { tokio::join!(caller, target) }).await?;
    sent?;

    // Then
    assert!(
        matches!(written, Err(ClientError::Failed(ServiceError::StdinClosed))),
        "{written:?}"
    );
    Ok(())
}

/// Writes `abcde` as stdin, closing it when `close`, against a target that answers `accept`,
/// takes the input when it admitted it, and then reports `reply`.
async fn stdin_answered(
    accept: Vec<u8>,
    reply: ProcessReply,
    close: bool,
) -> TestResult<Result<(u64, bool), ClientError>> {
    let ((mut reader, mut writer), service) = pipe();
    stdin_answered_over((&mut reader, &mut writer), service, (accept, reply, close)).await
}

/// As [`stdin_answered`], over the caller's own frame halves.
async fn stdin_answered_over<R, W>(
    (reader, writer): (
        &mut ma2a_services::FrameReader<R>,
        &mut ma2a_services::FrameWriter<W>,
    ),
    (mut service_reader, mut service_writer): support::ServerIo,
    (accept, reply, close): (Vec<u8>, ProcessReply, bool),
) -> TestResult<Result<(u64, bool), ClientError>>
where
    R: AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let target = async {
        service_writer.frame(FrameKind::Accept, &accept).await?;
        service_writer.flush().await?;
        if accept == ProcessAccept::Admitted.encode()? {
            let input = service_reader.expect().await?;
            assert_eq!(input.data()?, (Channel::Input, &b"abcde"[..]));
            assert_eq!(service_reader.expect().await?.end()?, Channel::Input);
        }
        let outcome = ProcessReply::encode_outcome(&Ok(reply))?;
        service_writer.frame(FrameKind::Outcome, &outcome).await?;
        service_writer.flush().await?;
        TestResult::Ok(())
    };
    let caller = hosted::stdin((reader, writer), (&b"abcde"[..], close));
    let (written, taken) = within(async { tokio::join!(caller, target) }).await?;
    taken?;
    Ok(written)
}

#[tokio::test]
async fn a_stdin_answer_must_admit_the_write_and_count_no_more_than_was_sent() -> TestResult {
    let admitted = ProcessAccept::Admitted.encode()?;
    for (reply, close) in [
        (ProcessReply::Written(5, false), false),
        (ProcessReply::Written(5, true), true),
        // The Process ended before it took every byte, which closed its stdin.
        (ProcessReply::Written(2, true), false),
    ] {
        let written = stdin_answered(admitted.clone(), reply.clone(), close).await?;
        assert!(written.is_ok(), "{reply:?}, close {close}: {written:?}");
    }

    let (_, reading) = reading(0, (0, 0))?;
    for (accept, reply, close) in [
        (reading, ProcessReply::Written(5, false), false),
        (admitted.clone(), ProcessReply::Written(100, false), false),
        // Still open, so it took every byte or it would have said why.
        (admitted.clone(), ProcessReply::Written(4, false), false),
        // Still open although closing was asked for.
        (admitted, ProcessReply::Written(5, false), true),
    ] {
        let written = stdin_answered(accept, reply.clone(), close).await?;
        assert!(
            matches!(
                written,
                Err(ClientError::Stream(ServiceError::InvalidInput))
            ),
            "{reply:?}, close {close}: {written:?}"
        );
    }
    Ok(())
}

/// A writer whose writes go through and whose flush always fails.
struct FlushFails<W>(W);

impl<W: tokio::io::AsyncWrite + Unpin> tokio::io::AsyncWrite for FlushFails<W> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.0).poll_write(context, buffer)
    }

    fn poll_flush(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()))
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_shutdown(context)
    }
}

#[tokio::test]
async fn a_stdin_answer_to_an_end_whose_flush_failed_still_counts_the_end() -> TestResult {
    // Given: a target that takes the input and its `end`, while the caller's flush fails.
    let ((mut reader, writer), service) = pipe();
    let mut writer = ma2a_services::FrameWriter::new(FlushFails(writer.into_inner()));
    let admitted = ProcessAccept::Admitted.encode()?;

    // When: the target reports every byte written and stdin still open.
    let written = stdin_answered_over(
        (&mut reader, &mut writer),
        service,
        (admitted, ProcessReply::Written(5, false), false),
    )
    .await?;

    // Then: that answers the `end` it received.
    assert_eq!(written?, (5, false));
    Ok(())
}

#[tokio::test]
async fn a_failed_local_read_ends_a_stdin_write_at_once() -> TestResult {
    // Given: a target that admits the write and then waits for more input.
    let ((mut reader, mut writer), (_service_reader, mut service_writer)) = pipe();
    let admitted = ProcessAccept::Admitted.encode()?;
    service_writer.frame(FrameKind::Accept, &admitted).await?;
    service_writer.flush().await?;

    // When: local input fails after one chunk.
    let written = tokio::time::timeout(
        Duration::from_secs(5),
        hosted::stdin((&mut reader, &mut writer), (FailsAfter(Some(b"ab")), true)),
    )
    .await?;

    // Then: the read failure is the answer; nothing waits for the target.
    assert!(matches!(written, Err(ClientError::Local(_))), "{written:?}");
    Ok(())
}

/// Writes `abcde` as stdin, closing it when `close`, against a target that answers with
/// `reply` alone, as a replay of a completed write does.
async fn stdin_replayed(
    reply: ProcessReply,
    close: bool,
) -> TestResult<Result<(u64, bool), ClientError>> {
    let ((mut reader, mut writer), (_service_reader, mut service_writer)) = pipe();
    let outcome = ProcessReply::encode_outcome(&Ok(reply))?;
    service_writer.frame(FrameKind::Outcome, &outcome).await?;
    service_writer.flush().await?;
    within(hosted::stdin(
        (&mut reader, &mut writer),
        (&b"abcde"[..], close),
    ))
    .await
}

#[tokio::test]
async fn a_replayed_stdin_answer_must_close_stdin_when_closing_was_asked_for() -> TestResult {
    // Given / When: replayed answers, whose byte count cannot be checked against this run.
    for (reply, close) in [
        (ProcessReply::Written(5, true), true),
        (ProcessReply::Written(3, false), false),
        (ProcessReply::Written(3, true), false),
    ] {
        // Then
        let written = stdin_replayed(reply.clone(), close).await?;
        assert!(written.is_ok(), "{reply:?}, close {close}: {written:?}");
    }

    // Then: a write that asked to close stdin never left it open.
    let written = stdin_replayed(ProcessReply::Written(0, false), true).await?;
    assert!(
        matches!(
            written,
            Err(ClientError::Stream(ServiceError::InvalidInput))
        ),
        "{written:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_detached_answer_only_completes_or_cancels() -> TestResult {
    for reason in [
        EndReason::Completed,
        EndReason::Cancelled,
        EndReason::TimedOut,
        EndReason::Shutdown,
    ] {
        // Given: a fresh answer and a replayed one, each detached with `reason`.
        let fresh = vec![reading(100, (0, 100))?, detached(reason, process(1), 100)?];
        let replayed = vec![detached(reason, process(1), 100)?];

        // When
        let (fresh, _) = attach(LogFrom::Cursor(100), None, fresh).await?;
        let (replayed, _) = attach(LogFrom::Now, Some(b""), replayed).await?;

        // Then: a timeout or shutdown is a failed outcome, never a Detached one.
        for attached in [fresh, replayed] {
            let allowed = matches!(reason, EndReason::Completed | EndReason::Cancelled);
            let refused = matches!(
                attached,
                Err(ClientError::Stream(ServiceError::InvalidInput))
            );
            assert!(allowed != refused, "{reason:?}: {attached:?}");
        }
    }
    Ok(())
}

#[tokio::test]
async fn a_failed_local_read_is_reported_when_the_outcome_comes_first() -> TestResult {
    // Given: local input that fails after one chunk, and a caller stream whose flush of the
    // detach is still pending when the target answers it.
    let wrap = |writer| Flushes(writer, 1);

    // When
    let (attached, kinds) =
        attach_writing_through(FailsAfter(Some(b"ab")), wrap, EndReason::Cancelled).await?;

    // Then: the failed read is still the answer.
    assert_eq!(kinds, [FrameKind::Data, FrameKind::Cancel]);
    assert!(
        matches!(attached, Err(ClientError::Local(_))),
        "{attached:?}"
    );
    Ok(())
}
