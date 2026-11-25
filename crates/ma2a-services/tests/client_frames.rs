//! Caller-side stream drivers against scripted targets that break the frame contract: an
//! accept with a payload, an `end` or `data` on a channel the target never sends on, and an
//! outcome counting other bytes than the run delivered.
//!
//! The target here is a test script writing exact Services v1 frames, not a host.

#[path = "support/host.rs"]
mod support;

use std::{
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};

use ma2a_core::service::{
    AgentAccept, Channel, ContentIdentity, EMPTY_ACCEPT, EndReason, FilesRequest, FilesResult,
    FrameKind, ProcessResult, ProxyResult, RemotePath, ServiceError, Termination, TransferAccept,
    WriteMode, encode_call_accept, encode_call_outcome,
};
use ma2a_services::{
    FrameReader, FrameWriter,
    client::{
        ClientError, agent, files,
        process::{self, ProcessControl, ProcessIo},
        proxy, service, transfer,
    },
};
use support::{Flushes, TestResult, pipe};
use tokio::io::AsyncWrite;

/// One frame a scripted target sends.
#[derive(Clone, Debug)]
enum Sent {
    Frame(FrameKind, Vec<u8>),
    Data(Channel, &'static [u8]),
    End(Channel),
}

/// The bytes a target sending `sent` writes.
async fn replies(sent: &[Sent]) -> TestResult<Vec<u8>> {
    let mut writer = FrameWriter::new(Vec::new());
    send(&mut writer, sent).await?;
    Ok(writer.into_inner())
}

/// Writes `sent` and flushes it.
async fn send<W: AsyncWrite + Unpin>(writer: &mut FrameWriter<W>, sent: &[Sent]) -> TestResult {
    for frame in sent {
        match frame {
            Sent::Frame(kind, payload) => writer.frame(*kind, payload).await?,
            Sent::Data(channel, bytes) => writer.data(*channel, bytes).await?,
            Sent::End(channel) => writer.end(*channel).await?,
        }
    }
    writer.flush().await?;
    Ok(())
}

fn accept(payload: &[u8]) -> Sent {
    Sent::Frame(FrameKind::Accept, payload.to_vec())
}

/// A payload other than the empty accept the families without a header send.
const HEADER: [u8; 2] = [0x81, 0x00];

const fn malformed<T>(result: &Result<T, ClientError>) -> bool {
    matches!(result, Err(ClientError::Stream(ServiceError::InvalidInput)))
}

const fn miscounted<T>(result: &Result<T, ClientError>) -> bool {
    matches!(
        result,
        Err(ClientError::Stream(ServiceError::IntegrityMismatch))
    )
}

/// A local sink that refuses every write, as a closed terminal or pipe does.
struct Broken;

impl AsyncWrite for Broken {
    fn poll_write(
        self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        _bytes: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()))
    }

    fn poll_flush(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        _context: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

async fn within<T>(future: impl std::future::Future<Output = T>) -> TestResult<T> {
    Ok(tokio::time::timeout(Duration::from_secs(30), future).await?)
}

/// The outcome of an Exec run that sent `bytes` of output and `errors` of stderr.
fn exited(bytes: u64, errors: u64) -> TestResult<Sent> {
    let result = ProcessResult::new(Termination::Exited(0), EndReason::Completed, 1)
        .with_bytes(bytes, errors);
    Ok(Sent::Frame(
        FrameKind::Outcome,
        ProcessResult::encode_outcome(&Ok(result))?,
    ))
}

/// Runs an Exec caller with no input against a target sending `sent`, its output going to
/// `output`.
async fn exec<O: AsyncWrite + Unpin>(
    sent: &[Sent],
    output: O,
) -> TestResult<Result<process::Streamed, ClientError>> {
    let replies = replies(sent).await?;
    let mut reader = FrameReader::new(replies.as_slice());
    let mut writer = FrameWriter::new(tokio::io::sink());
    let local = ProcessIo {
        input: None::<&[u8]>,
        output,
        error: Some(Vec::new()),
    };
    let control = ProcessControl {
        resizes: None,
        cancel: std::future::pending(),
    };
    within(process::run_streamed(
        (&mut reader, &mut writer),
        local,
        control,
    ))
    .await
}

#[tokio::test]
async fn an_exec_run_must_follow_the_frame_contract_and_count_what_it_sent() -> TestResult {
    let ran = exec(
        &[
            accept(&EMPTY_ACCEPT),
            Sent::Data(Channel::Output, b"abc"),
            Sent::Data(Channel::Error, b"e"),
            Sent::End(Channel::Output),
            Sent::End(Channel::Error),
            exited(3, 1)?,
        ],
        Vec::new(),
    )
    .await?;
    assert!(ran.is_ok(), "{ran:?}");

    let ran = exec(&[accept(&HEADER), exited(0, 0)?], Vec::new()).await?;
    assert!(malformed(&ran), "an accept with a header: {ran:?}");
    let ran = exec(
        &[
            accept(&EMPTY_ACCEPT),
            Sent::End(Channel::Input),
            exited(0, 0)?,
        ],
        Vec::new(),
    )
    .await?;
    assert!(malformed(&ran), "an end of the caller's input: {ran:?}");
    for (bytes, errors) in [(2, 1), (3, 0), (4, 1)] {
        let sent = [
            accept(&EMPTY_ACCEPT),
            Sent::Data(Channel::Output, b"abc"),
            Sent::Data(Channel::Error, b"e"),
            exited(bytes, errors)?,
        ];
        let ran = exec(&sent, Vec::new()).await?;
        assert!(miscounted(&ran), "{bytes} {errors}: {ran:?}");
    }
    Ok(())
}

#[tokio::test]
async fn an_exec_run_whose_output_was_lost_still_checks_every_frame() -> TestResult {
    let sent = [
        accept(&EMPTY_ACCEPT),
        Sent::Data(Channel::Output, b"abc"),
        Sent::Data(Channel::Input, b"x"),
        exited(3, 0)?,
    ];
    let ran = exec(&sent, Broken).await?;
    assert!(malformed(&ran), "{ran:?}");
    Ok(())
}

/// The outcome of a relay that took `to` bytes to its destination and `from` back.
fn relayed(to: u64, from: u64) -> TestResult<Sent> {
    let result = ProxyResult::new(EndReason::Completed, (to, from), 1);
    Ok(Sent::Frame(
        FrameKind::Outcome,
        ProxyResult::encode_outcome(&Ok(result))?,
    ))
}

/// Runs a Proxy caller sending `local` against a target sending `sent`; a relay completes
/// only once both sides ended, so the target holds its outcome until the caller's `end`.
async fn proxied(
    sent: &[Sent],
    local: &'static [u8],
) -> TestResult<Result<ProxyResult, ClientError>> {
    proxied_with(sent, local, true).await
}

/// Runs a Proxy caller reading `local` against a target sending `sent`, holding its outcome
/// until the caller's `end` when `after_end`.
async fn proxied_with<L: tokio::io::AsyncRead + Unpin>(
    sent: &[Sent],
    local: L,
    after_end: bool,
) -> TestResult<Result<ProxyResult, ClientError>> {
    let ((mut reader, mut writer), (mut target_reader, mut target_writer)) = pipe();
    let sent = sent.to_vec();
    let target = tokio::spawn(async move {
        for frame in &sent {
            if after_end && matches!(frame, Sent::Frame(FrameKind::Outcome, _)) {
                while target_reader.expect().await?.kind() != FrameKind::End {}
            }
            send(&mut target_writer, std::slice::from_ref(frame)).await?;
        }
        TestResult::Ok(())
    });
    let relayed = within(proxy::run((&mut reader, &mut writer), (local, Vec::new()))).await;
    target.abort();
    relayed
}

#[tokio::test]
async fn a_proxy_must_follow_the_frame_contract_and_count_what_it_relayed() -> TestResult {
    let from = Sent::Data(Channel::Input, b"abc");
    let ended = Sent::End(Channel::Input);
    let relay = [
        accept(&EMPTY_ACCEPT),
        from.clone(),
        ended.clone(),
        relayed(0, 3)?,
    ];
    let ran = proxied(&relay, b"").await?;
    assert!(ran.is_ok(), "{ran:?}");

    let ran = proxied(&[accept(&HEADER), relayed(0, 0)?], b"").await?;
    assert!(malformed(&ran), "an accept with a header: {ran:?}");
    for (case, sent) in [
        ("data after end", [ended.clone(), from.clone()]),
        ("end twice", [ended.clone(), ended.clone()]),
    ] {
        let sent = [accept(&EMPTY_ACCEPT), sent[0].clone(), sent[1].clone()];
        let ran = proxied(&[&sent[..], &[relayed(0, 3)?]].concat(), b"").await?;
        assert!(malformed(&ran), "{case}: {ran:?}");
    }
    for (to, from_target) in [(0, 2), (0, 4), (1, 3)] {
        let sent = [
            accept(&EMPTY_ACCEPT),
            from.clone(),
            ended.clone(),
            relayed(to, from_target)?,
        ];
        let ran = proxied(&sent, b"").await?;
        assert!(miscounted(&ran), "{to} {from_target}: {ran:?}");
    }
    Ok(())
}

/// Runs a service call with no input against a target sending `sent`, its output going to
/// `output`.
async fn called<O: AsyncWrite + Unpin>(
    sent: &[Sent],
    output: O,
) -> TestResult<Result<service::Called, ClientError>> {
    let replies = replies(sent).await?;
    let mut reader = FrameReader::new(replies.as_slice());
    let mut writer = FrameWriter::new(tokio::io::sink());
    let local = ProcessIo {
        input: None::<&[u8]>,
        output,
        error: Some(Vec::new()),
    };
    within(service::call(
        (&mut reader, &mut writer),
        local,
        std::future::pending(),
    ))
    .await
}

#[tokio::test]
async fn a_service_call_takes_output_only_on_the_services_channels() -> TestResult {
    let accepted = accept(&encode_call_accept(b"h")?);
    let outcome = Sent::Frame(FrameKind::Outcome, encode_call_outcome(Ok(b"r"))?);
    let sent = [accepted.clone(), Sent::End(Channel::Input), outcome.clone()];
    let ran = called(&sent, Vec::new()).await?;
    assert!(malformed(&ran), "an end of the caller's input: {ran:?}");
    let sent = [
        accepted,
        Sent::Data(Channel::Output, b"abc"),
        Sent::Data(Channel::Input, b"x"),
        outcome,
    ];
    let ran = called(&sent, Broken).await?;
    assert!(malformed(&ran), "input after a lost output: {ran:?}");
    Ok(())
}

#[tokio::test]
async fn a_files_answer_is_admitted_with_an_empty_accept() -> TestResult {
    let mkdir = FilesRequest::Mkdir(RemotePath::parse("docs:/b")?, false);
    let read = FilesRequest::Read(RemotePath::parse("docs:/a")?, 0, 16);
    let run = async |request: &FilesRequest, sent: &[Sent]| -> TestResult<_> {
        let replies = replies(sent).await?;
        let mut reader = FrameReader::new(replies.as_slice());
        let mut writer = FrameWriter::new(tokio::io::sink());
        within(files::run((&mut reader, &mut writer), request, &[])).await
    };
    // A content-free operation answers with its outcome alone.
    let done = Sent::Frame(
        FrameKind::Outcome,
        FilesResult::encode_outcome(&Ok(FilesResult::Done))?,
    );
    let made = run(&mkdir, std::slice::from_ref(&done)).await?;
    assert!(made.is_ok(), "{made:?}");
    // A read is accepted, with no header, before its content.
    let empty = Sent::Frame(
        FrameKind::Outcome,
        FilesResult::encode_outcome(&Ok(FilesResult::Read(0, true)))?,
    );
    let got = run(&read, &[accept(&EMPTY_ACCEPT), empty.clone()]).await?;
    assert!(got.is_ok(), "{got:?}");
    let got = run(&read, &[accept(&HEADER), empty.clone()]).await?;
    assert!(malformed(&got), "an accept with a header: {got:?}");
    let got = run(&read, &[empty]).await?;
    assert!(malformed(&got), "a read without accept: {got:?}");
    Ok(())
}

/// A `reject` after the target already admitted the operation.
fn late_reject(error: ServiceError) -> Sent {
    Sent::Frame(FrameKind::Reject, vec![error.code()])
}

#[tokio::test]
async fn a_reject_after_accept_is_a_broken_stream_not_a_refusal() -> TestResult {
    let late = late_reject(ServiceError::Conflict);
    let ran = exec(&[accept(&EMPTY_ACCEPT), late.clone()], Vec::new()).await?;
    assert!(malformed(&ran), "exec: {ran:?}");
    let call_accept = accept(&encode_call_accept(b"h")?);
    let ran = called(&[call_accept, late.clone()], Vec::new()).await?;
    assert!(malformed(&ran), "service call: {ran:?}");
    let ran = proxied(&[accept(&EMPTY_ACCEPT), late.clone()], b"").await?;
    assert!(malformed(&ran), "proxy: {ran:?}");
    let read = FilesRequest::Read(RemotePath::parse("docs:/a")?, 0, 16);
    let sent = replies(&[accept(&EMPTY_ACCEPT), late.clone()]).await?;
    let mut reader = FrameReader::new(sent.as_slice());
    let mut writer = FrameWriter::new(tokio::io::sink());
    let got = within(files::run((&mut reader, &mut writer), &read, &[])).await?;
    assert!(malformed(&got), "files read: {got:?}");
    let sent = replies(&[
        Sent::Frame(FrameKind::Accept, AgentAccept::Admitted.encode()?),
        late,
    ])
    .await?;
    let answered = within(agent::call(&mut FrameReader::new(sent.as_slice()))).await?;
    assert!(malformed(&answered), "agent call: {answered:?}");
    Ok(())
}

#[tokio::test]
async fn a_late_precondition_reject_of_a_resumed_download_keeps_it_resumable() -> TestResult {
    // Given: a resumed download the target accepted and started sending.
    let identity = ContentIdentity::new(10, 1);
    let sent = replies(&[
        Sent::Frame(FrameKind::Accept, TransferAccept::Get(identity).encode()?),
        Sent::Data(Channel::Output, b"abc"),
        late_reject(ServiceError::PreconditionFailed),
    ])
    .await?;
    let mut reader = FrameReader::new(sent.as_slice());
    let accepted = transfer::download_accepted(&mut reader).await?;

    // When: it ends with a refusal that says the content changed.
    let mut sink = Vec::new();
    let mut hasher = blake3::Hasher::new();
    let downloaded = within(transfer::download_content(
        &mut reader,
        (&mut sink, &mut hasher),
        (accepted, 4),
    ))
    .await?;

    // Then: that is a broken stream, not the refusal that would restart the download.
    assert!(malformed(&downloaded), "{downloaded:?}");
    Ok(())
}

#[tokio::test]
async fn output_that_could_not_be_written_fails_a_successful_run() -> TestResult {
    // Given: a local sink that refuses the output of a run the target reports as succeeded.
    let sent = [
        accept(&EMPTY_ACCEPT),
        Sent::Data(Channel::Output, b"abc"),
        exited(3, 0)?,
    ];

    // When
    let ran = exec(&sent, Broken).await?;

    // Then: the run is not reported as complete, since its output is not.
    assert!(matches!(ran, Err(ClientError::Local(_))), "exec: {ran:?}");
    let sent = [
        accept(&encode_call_accept(b"h")?),
        Sent::Data(Channel::Output, b"abc"),
        Sent::Frame(FrameKind::Outcome, encode_call_outcome(Ok(b"r"))?),
    ];
    let ran = called(&sent, Broken).await?;
    assert!(
        matches!(ran, Err(ClientError::Local(_))),
        "service call: {ran:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_target_sends_nothing_on_a_channel_it_ended() -> TestResult {
    let call_accept = accept(&encode_call_accept(b"h")?);
    let call_outcome = Sent::Frame(FrameKind::Outcome, encode_call_outcome(Ok(b"r"))?);
    let ended = Sent::End(Channel::Output);
    for (case, after) in [
        ("data after end", Sent::Data(Channel::Output, b"x")),
        ("end twice", ended.clone()),
    ] {
        let sent = [
            accept(&EMPTY_ACCEPT),
            ended.clone(),
            after.clone(),
            exited(1, 0)?,
        ];
        let ran = exec(&sent, Vec::new()).await?;
        assert!(malformed(&ran), "exec, {case}: {ran:?}");
        let sent = [
            call_accept.clone(),
            ended.clone(),
            after,
            call_outcome.clone(),
        ];
        let ran = called(&sent, Vec::new()).await?;
        assert!(malformed(&ran), "service call, {case}: {ran:?}");
    }
    Ok(())
}

/// Local input that delivers one chunk and then fails.
struct FailsAfter(Option<&'static [u8]>);

impl tokio::io::AsyncRead for FailsAfter {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        buffer: &mut tokio::io::ReadBuf<'_>,
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

/// The callers whose local input is forwarded as channel 0.
#[derive(Clone, Copy, Debug)]
enum Family {
    Exec,
    Shell,
    Call,
}

/// Runs `family`, whose local input fails after `ab`, against a target that takes that input
/// and the `cancel` after it, then sends `after` and ends the stream. Only the first
/// `flushes` flushes of the caller's stream finish.
async fn failing_input(
    family: Family,
    after: &[Sent],
    flushes: usize,
) -> TestResult<Result<(), ClientError>> {
    let ((mut reader, writer), (mut target_reader, mut target_writer)) = pipe();
    let mut writer = FrameWriter::new(Flushes(writer.into_inner(), flushes));
    let target = async move {
        let accepted = match family {
            Family::Call => encode_call_accept(b"")?,
            Family::Exec | Family::Shell => EMPTY_ACCEPT.to_vec(),
        };
        send(&mut target_writer, &[accept(&accepted)]).await?;
        assert_eq!(
            target_reader.expect().await?.data()?,
            (Channel::Input, &b"ab"[..])
        );
        assert_eq!(target_reader.expect().await?.kind(), FrameKind::Cancel);
        send(&mut target_writer, after).await?;
        target_writer.shutdown().await?;
        TestResult::Ok(())
    };
    let local = ProcessIo {
        input: Some(FailsAfter(Some(b"ab"))),
        output: Vec::new(),
        error: matches!(family, Family::Exec | Family::Call).then(Vec::new),
    };
    let (answer, scripted) = if matches!(family, Family::Call) {
        let calling = service::call((&mut reader, &mut writer), local, std::future::pending());
        let (called, scripted) = within(async { tokio::join!(calling, target) }).await?;
        (called.map(drop), scripted)
    } else {
        let control = ProcessControl {
            resizes: None,
            cancel: std::future::pending(),
        };
        let running = process::run((&mut reader, &mut writer), local, control);
        let (ran, scripted) = within(async { tokio::join!(running, target) }).await?;
        (ran.map(drop), scripted)
    };
    scripted?;
    Ok(answer)
}

#[tokio::test]
async fn a_broken_stream_is_reported_over_a_failed_local_read() -> TestResult {
    for family in [Family::Exec, Family::Call] {
        // A frame on the caller's own channel.
        let answer = failing_input(family, &[Sent::Data(Channel::Input, b"x")], usize::MAX).await?;
        assert!(malformed(&answer), "{family:?}: {answer:?}");
        // No terminal at all.
        let answer = failing_input(family, &[], usize::MAX).await?;
        assert!(
            matches!(
                answer,
                Err(ClientError::Stream(ServiceError::OutcomeUnknown))
            ),
            "{family:?}: {answer:?}"
        );
    }
    // An outcome counting output that never came.
    let answer = failing_input(Family::Exec, &[exited(5, 0)?], usize::MAX).await?;
    assert!(miscounted(&answer), "{answer:?}");
    // Any outcome the target did report gives way to the failed read.
    let answer = failing_input(Family::Exec, &[exited(0, 0)?], usize::MAX).await?;
    assert!(matches!(answer, Err(ClientError::Local(_))), "{answer:?}");
    Ok(())
}

#[tokio::test]
async fn a_failed_local_read_is_reported_when_the_outcome_comes_first() -> TestResult {
    let called = Sent::Frame(FrameKind::Outcome, encode_call_outcome(Ok(b"r"))?);
    for (family, outcome) in [
        (Family::Exec, exited(0, 0)?),
        (Family::Shell, exited(0, 0)?),
        (Family::Call, called),
    ] {
        // Given: local input that fails after one chunk, and a caller stream whose flush of
        // the `cancel` is still pending when the target answers it with success.
        // When
        let answer = failing_input(family, &[outcome], 1).await?;

        // Then: the failed read is still the answer.
        assert!(
            matches!(answer, Err(ClientError::Local(_))),
            "{family:?}: {answer:?}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn only_a_files_read_or_write_is_accepted_and_only_a_read_sends_content() -> TestResult {
    let run = async |request: &FilesRequest, content: &[u8], sent: &[Sent]| -> TestResult<_> {
        let sent = replies(sent).await?;
        let mut reader = FrameReader::new(sent.as_slice());
        let mut writer = FrameWriter::new(tokio::io::sink());
        within(files::run((&mut reader, &mut writer), request, content)).await
    };
    let done = Sent::Frame(
        FrameKind::Outcome,
        FilesResult::encode_outcome(&Ok(FilesResult::Done))?,
    );
    let mkdir = FilesRequest::Mkdir(RemotePath::parse("docs:/b")?, false);
    let made = run(&mkdir, &[], &[accept(&EMPTY_ACCEPT), done]).await?;
    assert!(malformed(&made), "an accepted mkdir: {made:?}");
    let write = FilesRequest::Write(RemotePath::parse("docs:/a")?, WriteMode::Replace, 1);
    let failed = Sent::Frame(
        FrameKind::Outcome,
        FilesResult::encode_outcome(&Err(ServiceError::Io))?,
    );
    let sent = [
        accept(&EMPTY_ACCEPT),
        Sent::Data(Channel::Output, b"x"),
        failed,
    ];
    let written = run(&write, b"y", &sent).await?;
    assert!(
        malformed(&written),
        "content answering a write: {written:?}"
    );
    Ok(())
}

/// The outcome of a relay that ended for `reason`, having taken `to` bytes to its destination
/// and `from` back.
fn relay_ended(reason: EndReason, (to, from): (u64, u64)) -> TestResult<Sent> {
    let result = ProxyResult::new(reason, (to, from), 1);
    Ok(Sent::Frame(
        FrameKind::Outcome,
        ProxyResult::encode_outcome(&Ok(result))?,
    ))
}

#[tokio::test]
async fn a_relay_completes_only_once_both_sides_ended_and_every_byte_was_taken() -> TestResult {
    let accepted = accept(&EMPTY_ACCEPT);
    let (from, ended) = (
        Sent::Data(Channel::Input, b"abc"),
        Sent::End(Channel::Input),
    );
    // Local input that never ends while the relay runs.
    let open = || tokio::io::duplex(64);

    // A relay completed at once, with the caller's input still open.
    let (input, _held) = open();
    let sent = [accepted.clone(), relay_ended(EndReason::Completed, (0, 0))?];
    let ran = proxied_with(&sent, input, false).await?;
    assert!(malformed(&ran), "premature: {ran:?}");
    // Only the destination ended.
    let (input, _held) = open();
    let sent = [
        accepted.clone(),
        ended.clone(),
        relay_ended(EndReason::Completed, (0, 0))?,
    ];
    let ran = proxied_with(&sent, input, false).await?;
    assert!(malformed(&ran), "destination ended: {ran:?}");
    // Only the caller ended.
    let sent = [accepted.clone(), relay_ended(EndReason::Completed, (0, 0))?];
    let ran = proxied(&sent, b"").await?;
    assert!(malformed(&ran), "caller ended: {ran:?}");
    // Both ended, but not every byte the caller sent reached the destination.
    let sent = [
        accepted.clone(),
        from.clone(),
        ended.clone(),
        relay_ended(EndReason::Completed, (1, 3))?,
    ];
    let ran = proxied(&sent, b"hi").await?;
    assert!(miscounted(&ran), "short: {ran:?}");

    // Both ended with every byte relayed.
    let sent = [
        accepted.clone(),
        from.clone(),
        ended,
        relay_ended(EndReason::Completed, (2, 3))?,
    ];
    let ran = proxied(&sent, b"hi").await?;
    assert!(ran.is_ok(), "completed: {ran:?}");
    // A cancelled relay may have moved part of the traffic.
    let (input, _held) = open();
    let sent = [accepted, from, relay_ended(EndReason::Cancelled, (0, 3))?];
    let ran = proxied_with(&sent, input, false).await?;
    assert!(ran.is_ok(), "cancelled: {ran:?}");
    Ok(())
}

/// Where a local sink first fails, if anywhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FailsOn {
    Write,
    Flush,
    Shutdown,
    Nothing,
}

/// A local sink that keeps what it was given and fails at one step.
struct Sink(FailsOn, Vec<u8>);

impl Sink {
    fn step(&self, step: FailsOn) -> Poll<std::io::Result<()>> {
        if self.0 == step {
            Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()))
        } else {
            Poll::Ready(Ok(()))
        }
    }
}

impl AsyncWrite for Sink {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        if self.0 == FailsOn::Write {
            return Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()));
        }
        self.1.extend_from_slice(bytes);
        Poll::Ready(Ok(bytes.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        self.step(FailsOn::Flush)
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        _context: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        self.step(FailsOn::Shutdown)
    }
}

/// Runs a Proxy caller with no local input, its destination's bytes going to `sink`, against
/// a target that takes the caller's `end`, then relays `abc` from the destination, ends it,
/// and reports the relay complete.
async fn proxied_into(sink: &mut Sink) -> TestResult<Result<ProxyResult, ClientError>> {
    let ((mut reader, mut writer), (mut target_reader, mut target_writer)) = pipe();
    let target = async {
        send(&mut target_writer, &[accept(&EMPTY_ACCEPT)]).await?;
        assert_eq!(target_reader.expect().await?.end()?, Channel::Input);
        let relay = [
            Sent::Data(Channel::Input, b"abc"),
            Sent::End(Channel::Input),
            relayed(0, 3)?,
        ];
        send(&mut target_writer, &relay).await
    };
    let relaying = proxy::run((&mut reader, &mut writer), (&b""[..], sink));
    let (relayed, scripted) = within(async { tokio::join!(relaying, target) }).await?;
    scripted?;
    Ok(relayed)
}

#[tokio::test]
async fn a_relay_whose_bytes_could_not_be_delivered_locally_is_not_completed() -> TestResult {
    for fails in [
        FailsOn::Write,
        FailsOn::Flush,
        FailsOn::Shutdown,
        FailsOn::Nothing,
    ] {
        // Given: a local side that fails to take the destination's bytes, or their end.
        let mut sink = Sink(fails, Vec::new());

        // When: the target reports the relay complete.
        let ran = proxied_into(&mut sink).await?;

        // Then: only a sink that took every byte makes a completed relay.
        if fails == FailsOn::Nothing {
            assert!(ran.is_ok(), "{ran:?}");
            assert_eq!(sink.1, b"abc");
        } else {
            assert!(
                matches!(ran, Err(ClientError::Local(_))),
                "{fails:?}: {ran:?}"
            );
        }
    }
    Ok(())
}
