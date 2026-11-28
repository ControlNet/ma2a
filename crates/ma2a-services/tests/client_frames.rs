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
    let relayed = within(proxy::run(
        (&mut reader, &mut writer),
        (local, Vec::new()),
        (Duration::from_secs(60), std::future::pending()),
    ))
    .await;
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
    let relaying = proxy::run(
        (&mut reader, &mut writer),
        (&b""[..], sink),
        (Duration::from_secs(60), std::future::pending()),
    );
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

/// Continuously floods 64-byte `data` frames on `channel` until `stop` is cancelled.
fn flood_data<W: AsyncWrite + Unpin + Send + 'static>(
    mut writer: FrameWriter<W>,
    channel: Channel,
    stop: tokio_util::sync::CancellationToken,
) -> tokio::task::JoinHandle<TestResult> {
    use tokio::io::AsyncWriteExt as _;
    tokio::spawn(async move {
        let mut batch = FrameWriter::new(Vec::new());
        for _ in 0..256 {
            batch.data(channel, &[b'x'; 64]).await?;
        }
        let batch = batch.into_inner();
        loop {
            tokio::select! {
                () = stop.cancelled() => return Ok(()),
                sent = writer.get_mut().write_all(&batch) => {
                    if sent.is_err() {
                        return Ok(());
                    }
                }
            }
        }
    })
}

/// Runs `caller` under a continuous incoming `data` flood on `channel` after `accept` and
/// asserts the peer sees one frame satisfying `check` within 2 seconds.
async fn assert_fair_under_flood<F, Fut, C>(
    (accept, channel): (&[u8], Channel),
    drive: F,
    check: C,
) -> TestResult
where
    F: FnOnce(
        FrameReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>,
        FrameWriter<tokio::io::WriteHalf<tokio::io::DuplexStream>>,
    ) -> Fut,
    Fut: std::future::Future<Output = TestResult>,
    C: FnOnce(&ma2a_services::Frame) -> TestResult,
{
    let ((reader, writer), (mut peer_reader, mut peer_writer)) = pipe();
    peer_writer.frame(FrameKind::Accept, accept).await?;
    peer_writer.flush().await?;
    let stop = tokio_util::sync::CancellationToken::new();
    let flood = flood_data(peer_writer, channel, stop.clone());
    let caller = drive(reader, writer);
    let peer = async {
        let frame = tokio::time::timeout(Duration::from_secs(2), peer_reader.expect()).await??;
        check(&frame)?;
        stop.cancel();
        TestResult::Ok(())
    };
    let result = tokio::select! {
        result = peer => result,
        _ = caller => Err("caller ended before peer saw outgoing frame".into()),
    };
    flood.abort();
    result
}

/// A writer whose first write waits until `delay` elapses, forcing `first_or_cancel` to hand
/// the pending `cancel` off to `drained`.
struct DelayedWrite<W> {
    inner: W,
    delay: Pin<Box<tokio::time::Sleep>>,
    ready: bool,
}

impl<W: AsyncWrite + Unpin> AsyncWrite for DelayedWrite<W> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        if !self.ready {
            if self.delay.as_mut().poll(context).is_pending() {
                return Poll::Pending;
            }
            self.ready = true;
        }
        Pin::new(&mut self.inner).poll_write(context, buf)
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(context)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(context)
    }
}

/// Checks that a pre-accept `cancel` still flushes via `drained` while the peer floods `data`.
async fn assert_drained_cancel_fair_under_flood() -> TestResult {
    use tokio::io::AsyncWriteExt as _;
    let ((mut reader, writer), (mut peer_reader, mut peer_writer)) = pipe();
    let mut writer = FrameWriter::new(DelayedWrite {
        inner: writer.into_inner(),
        delay: Box::pin(tokio::time::sleep(Duration::from_millis(30))),
        ready: false,
    });
    let stop = tokio_util::sync::CancellationToken::new();
    let stop_flood = stop.clone();
    let flood = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        peer_writer.frame(FrameKind::Accept, &EMPTY_ACCEPT).await?;
        peer_writer.flush().await?;
        let mut batch = FrameWriter::new(Vec::new());
        for _ in 0..256 {
            batch.data(Channel::Output, &[b'x'; 64]).await?;
        }
        let batch = batch.into_inner();
        loop {
            tokio::select! {
                () = stop_flood.cancelled() => return TestResult::Ok(()),
                sent = peer_writer.get_mut().write_all(&batch) => {
                    if sent.is_err() {
                        return Ok(());
                    }
                }
            }
        }
    });
    let local = ProcessIo {
        input: None::<&[u8]>,
        output: tokio::io::sink(),
        error: Some(tokio::io::sink()),
    };
    let control = ProcessControl {
        resizes: None,
        cancel: async {},
    };
    let caller = process::run_streamed((&mut reader, &mut writer), local, control);
    let peer = async {
        let frame = tokio::time::timeout(Duration::from_secs(2), peer_reader.expect()).await??;
        assert_eq!(frame.kind(), FrameKind::Cancel);
        stop.cancel();
        TestResult::Ok(())
    };
    let result = tokio::select! {
        result = peer => result,
        _ = caller => Err("drained caller ended before peer saw cancel".into()),
    };
    flood.abort();
    result
}

/// Drives a continuous incoming `data` stream and checks that Exec, service call, and attach
/// outgoing cancel/detach frames still reach the peer without starvation.
#[tokio::test]
async fn a_continuous_incoming_stream_never_starves_exec_service_or_attach() -> TestResult {
    use ma2a_core::service::{LogFrom, LogWindow, ProcessAccept, ProcessId, ProcessState};
    use ma2a_services::client::hosted;

    let is_cancel = |frame: &ma2a_services::Frame| -> TestResult {
        assert_eq!(frame.kind(), FrameKind::Cancel);
        Ok(())
    };
    // 1. Exec: cancel during continuous stdout flood.
    assert_fair_under_flood(
        (&EMPTY_ACCEPT, Channel::Output),
        |mut reader, mut writer| async move {
            let local = ProcessIo {
                input: None::<&[u8]>,
                output: tokio::io::sink(),
                error: Some(tokio::io::sink()),
            };
            let control = ProcessControl {
                resizes: None,
                cancel: tokio::time::sleep(Duration::from_millis(20)),
            };
            let _ = process::run_streamed((&mut reader, &mut writer), local, control).await;
            Ok(())
        },
        is_cancel,
    )
    .await?;
    // 2. Exec: BrokenPipe on local output sends cancel during continuous stdout flood.
    assert_fair_under_flood(
        (&EMPTY_ACCEPT, Channel::Output),
        |mut reader, mut writer| async move {
            let local = ProcessIo {
                input: None::<&[u8]>,
                output: Broken,
                error: Some(tokio::io::sink()),
            };
            let control = ProcessControl {
                resizes: None,
                cancel: std::future::pending(),
            };
            let _ = process::run_streamed((&mut reader, &mut writer), local, control).await;
            Ok(())
        },
        is_cancel,
    )
    .await?;
    // 3. Service call: cancel during continuous output flood.
    assert_fair_under_flood(
        (&encode_call_accept(b"")?, Channel::Output),
        |mut reader, mut writer| async move {
            let local = ProcessIo {
                input: None::<&[u8]>,
                output: tokio::io::sink(),
                error: Some(tokio::io::sink()),
            };
            let _ = service::call(
                (&mut reader, &mut writer),
                local,
                tokio::time::sleep(Duration::from_millis(20)),
            )
            .await;
            Ok(())
        },
        is_cancel,
    )
    .await?;
    // 4. Hosted attach: detach during continuous output flood.
    let reading = ProcessAccept::Reading {
        state: ProcessState::Running,
        start: 0,
        window: LogWindow::new(0, 0),
        truncated: false,
    }
    .encode()?;
    assert_fair_under_flood(
        (&reading, Channel::Output),
        |mut reader, mut writer| async move {
            let local = ProcessIo {
                input: None::<&[u8]>,
                output: tokio::io::sink(),
                error: None::<tokio::io::Sink>,
            };
            let _ = hosted::attach(
                (&mut reader, &mut writer),
                ((ProcessId::from_bytes([1; 16]), LogFrom::Now), local),
                tokio::time::sleep(Duration::from_millis(20)),
            )
            .await;
            Ok(())
        },
        is_cancel,
    )
    .await
}

/// Drives a continuous incoming `data` stream and checks that Proxy upstream data, `BrokenPipe`
/// cancel, and a pre-accept `cancel` flushing via `drained` are never starved.
#[tokio::test]
async fn a_continuous_incoming_stream_never_starves_proxy_or_drained_cancel() -> TestResult {
    let is_cancel = |frame: &ma2a_services::Frame| -> TestResult {
        assert_eq!(frame.kind(), FrameKind::Cancel);
        Ok(())
    };
    assert_fair_under_flood(
        (&EMPTY_ACCEPT, Channel::Input),
        |mut reader, mut writer| async move {
            let _ = proxy::run(
                (&mut reader, &mut writer),
                (&b"hello"[..], tokio::io::sink()),
                (Duration::from_secs(60), std::future::pending()),
            )
            .await;
            Ok(())
        },
        |frame| {
            assert_eq!(frame.data()?, (Channel::Input, &b"hello"[..]));
            Ok(())
        },
    )
    .await?;
    assert_fair_under_flood(
        (&EMPTY_ACCEPT, Channel::Input),
        |mut reader, mut writer| async move {
            let (_hold, local_read) = tokio::io::duplex(64);
            let _ = proxy::run(
                (&mut reader, &mut writer),
                (local_read, Broken),
                (Duration::from_secs(60), std::future::pending()),
            )
            .await;
            Ok(())
        },
        is_cancel,
    )
    .await?;
    assert_drained_cancel_fair_under_flood().await
}

/// A local sink whose writes and flushes succeed, but whose shutdown stays pending forever.
struct StalledShutdown;

impl AsyncWrite for StalledShutdown {
    fn poll_write(
        self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Poll::Ready(Ok(bytes.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        _context: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Poll::Pending
    }
}

#[tokio::test(start_paused = true)]
async fn a_stalled_local_proxy_consumer_fails_by_the_idle_deadline() -> TestResult {
    let idle = Duration::from_secs(5);

    // 1. One-byte non-draining sink with target data + TimedOut terminal + close.
    let ((mut reader, mut writer), (_target_reader, mut target_writer)) = pipe();
    let (_stalled_consumer, local_write) = tokio::io::duplex(1);
    let (_hold, local_read) = tokio::io::duplex(64);
    send(
        &mut target_writer,
        &[
            accept(&EMPTY_ACCEPT),
            Sent::Data(Channel::Input, b"abcdef"),
            relay_ended(EndReason::TimedOut, (0, 6))?,
        ],
    )
    .await?;
    drop(target_writer);
    let started = tokio::time::Instant::now();
    let result = tokio::time::timeout(
        idle * 2,
        proxy::run(
            (&mut reader, &mut writer),
            (local_read, local_write),
            (idle, std::future::pending()),
        ),
    )
    .await?;
    assert_eq!(started.elapsed(), idle);
    assert!(
        matches!(result, Err(ClientError::Local(ref error)) if error.kind() == std::io::ErrorKind::TimedOut),
        "{result:?}"
    );

    // 2. Live target receives Cancel once the stalled consumer times out, and answers Cancelled.
    let ((mut reader, mut writer), (mut target_reader, mut target_writer)) = pipe();
    let (_stalled_consumer, local_write) = tokio::io::duplex(1);
    let (_hold, local_read) = tokio::io::duplex(64);
    let target = async {
        send(
            &mut target_writer,
            &[accept(&EMPTY_ACCEPT), Sent::Data(Channel::Input, b"abcdef")],
        )
        .await?;
        assert_eq!(target_reader.expect().await?.kind(), FrameKind::Cancel);
        send(
            &mut target_writer,
            &[relay_ended(EndReason::Cancelled, (0, 6))?],
        )
        .await
    };
    let started = tokio::time::Instant::now();
    let caller = proxy::run(
        (&mut reader, &mut writer),
        (local_read, local_write),
        (idle, std::future::pending()),
    );
    let (result, scripted) =
        tokio::time::timeout(idle * 2, async { tokio::join!(caller, target) }).await?;
    scripted?;
    assert_eq!(started.elapsed(), idle);
    assert!(
        matches!(result, Err(ClientError::Local(ref error)) if error.kind() == std::io::ErrorKind::TimedOut),
        "{result:?}"
    );

    // 3. Stalled local shutdown fails at the idle deadline.
    let ((mut reader, mut writer), (mut target_reader, mut target_writer)) = pipe();
    let target = async {
        send(&mut target_writer, &[accept(&EMPTY_ACCEPT)]).await?;
        assert_eq!(target_reader.expect().await?.kind(), FrameKind::End);
        send(
            &mut target_writer,
            &[
                Sent::Data(Channel::Input, b"abc"),
                Sent::End(Channel::Input),
                relayed(0, 3)?,
            ],
        )
        .await
    };
    let started = tokio::time::Instant::now();
    let caller = proxy::run(
        (&mut reader, &mut writer),
        (&b""[..], StalledShutdown),
        (idle, std::future::pending()),
    );
    let (result, scripted) =
        tokio::time::timeout(idle * 2, async { tokio::join!(caller, target) }).await?;
    scripted?;
    assert_eq!(started.elapsed(), idle);
    assert!(
        matches!(result, Err(ClientError::Local(ref error)) if error.kind() == std::io::ErrorKind::TimedOut),
        "{result:?}"
    );
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_slow_local_proxy_consumer_making_progress_receives_the_complete_stream() -> TestResult {
    use tokio::io::AsyncReadExt as _;

    // Given: a one-byte sink drained one byte every 2 s under a 5 s idle budget, so the total
    // 8-byte transfer exceeds one idle interval while every individual step makes progress.
    let idle = Duration::from_secs(5);
    let ((mut reader, mut writer), (mut target_reader, mut target_writer)) = pipe();
    let (mut consumer, local_write) = tokio::io::duplex(1);
    let target = async {
        send(&mut target_writer, &[accept(&EMPTY_ACCEPT)]).await?;
        assert_eq!(target_reader.expect().await?.kind(), FrameKind::End);
        send(
            &mut target_writer,
            &[
                Sent::Data(Channel::Input, b"abcdefgh"),
                Sent::End(Channel::Input),
                relayed(0, 8)?,
            ],
        )
        .await
    };
    let drain = async {
        let mut received = Vec::new();
        let mut byte = [0_u8; 1];
        loop {
            tokio::time::sleep(Duration::from_secs(2)).await;
            if consumer.read(&mut byte).await? == 0 {
                return TestResult::Ok(received);
            }
            received.push(byte[0]);
        }
    };
    let caller = proxy::run(
        (&mut reader, &mut writer),
        (&b""[..], local_write),
        (idle, std::future::pending()),
    );
    let (result, scripted, received) = tokio::time::timeout(Duration::from_secs(60), async {
        tokio::join!(caller, target, drain)
    })
    .await?;
    scripted?;
    assert_eq!(result?.reason, EndReason::Completed);
    assert_eq!(received?, b"abcdefgh");
    Ok(())
}

async fn run_blocked_process_sink<O: AsyncWrite + Unpin, E: AsyncWrite + Unpin>(
    local: ProcessIo<&'static [u8], O, E>,
    channel: Channel,
    counts: (u64, u64),
) -> TestResult {
    let ((mut reader, mut writer), (mut target_reader, mut target_writer)) = pipe();
    let outcome = {
        let mut result = ProcessResult::new(Termination::Exited(0), EndReason::Cancelled, 1);
        result.output_bytes = counts.0;
        result.error_bytes = counts.1;
        ProcessResult::encode_outcome(&Ok(result))?
    };
    let target = async {
        send(
            &mut target_writer,
            &[accept(&EMPTY_ACCEPT), Sent::Data(channel, b"abcdef")],
        )
        .await?;
        assert_eq!(target_reader.expect().await?.kind(), FrameKind::Cancel);
        send(
            &mut target_writer,
            &[
                Sent::Data(channel, b"xyz"),
                Sent::End(channel),
                Sent::Frame(FrameKind::Outcome, outcome),
            ],
        )
        .await
    };
    let started = tokio::time::Instant::now();
    let caller = process::run_streamed(
        (&mut reader, &mut writer),
        local,
        ProcessControl {
            resizes: None,
            cancel: tokio::time::sleep(Duration::from_millis(50)),
        },
    );
    let (result, scripted) = tokio::time::timeout(Duration::from_secs(2), async {
        tokio::join!(caller, target)
    })
    .await?;
    scripted?;
    assert!(started.elapsed() < Duration::from_secs(1));
    let streamed = result?;
    assert_eq!(streamed.result.reason, EndReason::Cancelled);
    assert_eq!(
        (streamed.result.output_bytes, streamed.result.error_bytes),
        counts
    );
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_cancelled_process_interrupts_blocked_stdout_and_stderr_and_validates_discarded_frames()
-> TestResult {
    let outcome = |reason, out_bytes, err_bytes| -> TestResult<Vec<u8>> {
        let mut result = ProcessResult::new(Termination::Exited(0), reason, 1);
        result.output_bytes = out_bytes;
        result.error_bytes = err_bytes;
        Ok(ProcessResult::encode_outcome(&Ok(result))?)
    };

    let (_hold_out, out_sink) = tokio::io::duplex(1);
    run_blocked_process_sink(
        ProcessIo {
            input: None,
            output: out_sink,
            error: Some(tokio::io::sink()),
        },
        Channel::Output,
        (9, 0),
    )
    .await?;

    let (_hold_err, err_sink) = tokio::io::duplex(1);
    run_blocked_process_sink(
        ProcessIo {
            input: None,
            output: tokio::io::sink(),
            error: Some(err_sink),
        },
        Channel::Error,
        (0, 9),
    )
    .await?;

    // 2. Discarded frames after cancel still reject mismatched byte counts.
    let ((mut reader, mut writer), (mut target_reader, mut target_writer)) = pipe();
    let (_hold, out_sink) = tokio::io::duplex(1);
    let target = async {
        send(
            &mut target_writer,
            &[
                accept(&EMPTY_ACCEPT),
                Sent::Data(Channel::Output, b"abcdef"),
            ],
        )
        .await?;
        assert_eq!(target_reader.expect().await?.kind(), FrameKind::Cancel);
        send(
            &mut target_writer,
            &[
                Sent::Data(Channel::Output, b"xyz"),
                Sent::Frame(FrameKind::Outcome, outcome(EndReason::Cancelled, 6, 0)?),
            ],
        )
        .await
    };
    let caller = process::run_streamed(
        (&mut reader, &mut writer),
        ProcessIo {
            input: None::<&[u8]>,
            output: out_sink,
            error: Some(tokio::io::sink()),
        },
        ProcessControl {
            resizes: None,
            cancel: tokio::time::sleep(Duration::from_millis(50)),
        },
    );
    let (result, scripted) = tokio::time::timeout(Duration::from_secs(2), async {
        tokio::join!(caller, target)
    })
    .await?;
    scripted?;
    assert!(miscounted(&result), "{result:?}");
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_cancelled_process_bounds_a_missing_terminal() -> TestResult {
    // A peer that acknowledges nothing after Cancel is bounded by FRAME_GRACE.
    let ((mut reader, mut writer), (mut target_reader, mut target_writer)) = pipe();
    let (_hold, out_sink) = tokio::io::duplex(1);
    let target = async {
        send(
            &mut target_writer,
            &[
                accept(&EMPTY_ACCEPT),
                Sent::Data(Channel::Output, b"abcdef"),
            ],
        )
        .await?;
        assert_eq!(target_reader.expect().await?.kind(), FrameKind::Cancel);
        TestResult::Ok(())
    };
    let started = tokio::time::Instant::now();
    let caller = process::run_streamed(
        (&mut reader, &mut writer),
        ProcessIo {
            input: None::<&[u8]>,
            output: out_sink,
            error: Some(tokio::io::sink()),
        },
        ProcessControl {
            resizes: None,
            cancel: tokio::time::sleep(Duration::from_millis(50)),
        },
    );
    let (result, scripted) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(caller, target)
    })
    .await?;
    scripted?;
    assert_eq!(
        started.elapsed(),
        Duration::from_millis(50) + ma2a_services::reply::FRAME_GRACE
    );
    assert!(
        matches!(
            result,
            Err(ClientError::Stream(ServiceError::OutcomeUnknown))
        ),
        "{result:?}"
    );
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn an_uncancelled_slow_process_consumer_receives_every_byte_in_order() -> TestResult {
    use tokio::io::AsyncReadExt as _;

    let ((mut reader, mut writer), (_target_reader, mut target_writer)) = pipe();
    let (mut consumer, out_sink) = tokio::io::duplex(1);
    let mut completed = ProcessResult::new(Termination::Exited(0), EndReason::Completed, 1);
    completed.output_bytes = 8;
    send(
        &mut target_writer,
        &[
            accept(&EMPTY_ACCEPT),
            Sent::Data(Channel::Output, b"abcdefgh"),
            Sent::End(Channel::Output),
            Sent::Frame(
                FrameKind::Outcome,
                ProcessResult::encode_outcome(&Ok(completed))?,
            ),
        ],
    )
    .await?;
    drop(target_writer);

    let drain = async {
        let mut received = Vec::new();
        let mut byte = [0_u8; 1];
        loop {
            tokio::time::sleep(Duration::from_millis(500)).await;
            if consumer.read(&mut byte).await? == 0 {
                return TestResult::Ok(received);
            }
            received.push(byte[0]);
        }
    };
    let caller = process::run_streamed(
        (&mut reader, &mut writer),
        ProcessIo {
            input: None::<&[u8]>,
            output: out_sink,
            error: None::<tokio::io::Sink>,
        },
        ProcessControl {
            resizes: None,
            cancel: std::future::pending(),
        },
    );
    let (result, received) = tokio::time::timeout(Duration::from_secs(30), async {
        tokio::join!(caller, drain)
    })
    .await?;
    let streamed = result?;
    assert_eq!(streamed.result.reason, EndReason::Completed);
    assert_eq!(received?, b"abcdefgh");
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_cancelled_hosted_call_interrupts_blocked_local_sinks_and_bounds_missing_terminal()
-> TestResult {
    let ((mut reader, mut writer), (mut target_reader, mut target_writer)) = pipe();
    let (_hold, out_sink) = tokio::io::duplex(1);
    let target = async {
        send(
            &mut target_writer,
            &[
                accept(&encode_call_accept(b"")?),
                Sent::Data(Channel::Output, b"abcdef"),
            ],
        )
        .await?;
        assert_eq!(target_reader.expect().await?.kind(), FrameKind::Cancel);
        send(
            &mut target_writer,
            &[Sent::Frame(
                FrameKind::Outcome,
                encode_call_outcome(Err(ServiceError::Cancelled))?,
            )],
        )
        .await
    };
    let caller = service::call(
        (&mut reader, &mut writer),
        ProcessIo {
            input: None::<&[u8]>,
            output: out_sink,
            error: Some(tokio::io::sink()),
        },
        tokio::time::sleep(Duration::from_millis(50)),
    );
    let (result, scripted) = tokio::time::timeout(Duration::from_secs(2), async {
        tokio::join!(caller, target)
    })
    .await?;
    scripted?;
    assert!(
        matches!(result, Err(ClientError::Failed(ServiceError::Cancelled))),
        "{result:?}"
    );
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn cancelling_a_proxy_with_a_blocked_sink_or_shutdown_returns_cancelled_promptly()
-> TestResult {
    let outcome = |reason, from_target| -> TestResult<Vec<u8>> {
        let result = ProxyResult::new(reason, (0, from_target), 1);
        Ok(ProxyResult::encode_outcome(&Ok(result))?)
    };

    // 1. Full one-byte local sink under a 60 s idle budget, cancelled at 1 s.
    let ((mut reader, mut writer), (mut target_reader, mut target_writer)) = pipe();
    let (_hold_consumer, local_write) = tokio::io::duplex(1);
    let (_hold_input, local_read) = tokio::io::duplex(64);
    let target = async {
        send(
            &mut target_writer,
            &[accept(&EMPTY_ACCEPT), Sent::Data(Channel::Input, b"abcdef")],
        )
        .await?;
        assert_eq!(target_reader.expect().await?.kind(), FrameKind::Cancel);
        send(
            &mut target_writer,
            &[Sent::Frame(
                FrameKind::Outcome,
                outcome(EndReason::Cancelled, 6)?,
            )],
        )
        .await
    };
    let started = tokio::time::Instant::now();
    let caller = proxy::run(
        (&mut reader, &mut writer),
        (local_read, local_write),
        (
            Duration::from_secs(60),
            tokio::time::sleep(Duration::from_secs(1)),
        ),
    );
    let (result, scripted) = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(caller, target)
    })
    .await?;
    scripted?;
    assert_eq!(started.elapsed(), Duration::from_secs(1));
    assert_eq!(result?.reason, EndReason::Cancelled);

    // 2. Stalled local shutdown under a 60 s idle budget, cancelled at 1 s.
    let ((mut reader, mut writer), (mut target_reader, mut target_writer)) = pipe();
    let target = async {
        send(
            &mut target_writer,
            &[
                accept(&EMPTY_ACCEPT),
                Sent::Data(Channel::Input, b"abc"),
                Sent::End(Channel::Input),
            ],
        )
        .await?;
        assert_eq!(target_reader.expect().await?.kind(), FrameKind::End);
        assert_eq!(target_reader.expect().await?.kind(), FrameKind::Cancel);
        send(
            &mut target_writer,
            &[Sent::Frame(
                FrameKind::Outcome,
                outcome(EndReason::Cancelled, 3)?,
            )],
        )
        .await
    };
    let started = tokio::time::Instant::now();
    let caller = proxy::run(
        (&mut reader, &mut writer),
        (&b""[..], StalledShutdown),
        (
            Duration::from_secs(60),
            tokio::time::sleep(Duration::from_secs(1)),
        ),
    );
    let (result, scripted) = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(caller, target)
    })
    .await?;
    scripted?;
    assert_eq!(started.elapsed(), Duration::from_secs(1));
    assert_eq!(result?.reason, EndReason::Cancelled);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn cancelling_a_proxy_bounds_missing_terminal_and_preserves_prior_io_error() -> TestResult {
    let outcome = |reason, from_target| -> TestResult<Vec<u8>> {
        let result = ProxyResult::new(reason, (0, from_target), 1);
        Ok(ProxyResult::encode_outcome(&Ok(result))?)
    };

    // 1. Missing terminal after cancelling a blocked proxy sink is bounded by FRAME_GRACE.
    let ((mut reader, mut writer), (mut target_reader, mut target_writer)) = pipe();
    let (_hold_consumer, local_write) = tokio::io::duplex(1);
    let (_hold_input, local_read) = tokio::io::duplex(64);
    let target = async {
        send(
            &mut target_writer,
            &[accept(&EMPTY_ACCEPT), Sent::Data(Channel::Input, b"abcdef")],
        )
        .await?;
        assert_eq!(target_reader.expect().await?.kind(), FrameKind::Cancel);
        TestResult::Ok(())
    };
    let started = tokio::time::Instant::now();
    let caller = proxy::run(
        (&mut reader, &mut writer),
        (local_read, local_write),
        (
            Duration::from_secs(60),
            tokio::time::sleep(Duration::from_secs(1)),
        ),
    );
    let (result, scripted) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(caller, target)
    })
    .await?;
    scripted?;
    assert_eq!(
        started.elapsed(),
        Duration::from_secs(1) + ma2a_services::reply::FRAME_GRACE
    );
    assert!(
        matches!(
            result,
            Err(ClientError::Stream(ServiceError::OutcomeUnknown))
        ),
        "{result:?}"
    );

    // 2. A local idle timeout before cancellation preserves the local I/O error.
    let ((mut reader, mut writer), (mut target_reader, mut target_writer)) = pipe();
    let (_hold_consumer, local_write) = tokio::io::duplex(1);
    let (_hold_input, local_read) = tokio::io::duplex(64);
    let target = async {
        send(
            &mut target_writer,
            &[accept(&EMPTY_ACCEPT), Sent::Data(Channel::Input, b"abcdef")],
        )
        .await?;
        assert_eq!(target_reader.expect().await?.kind(), FrameKind::Cancel);
        send(
            &mut target_writer,
            &[Sent::Frame(
                FrameKind::Outcome,
                outcome(EndReason::Cancelled, 6)?,
            )],
        )
        .await
    };
    let caller = proxy::run(
        (&mut reader, &mut writer),
        (local_read, local_write),
        (
            Duration::from_secs(1),
            tokio::time::sleep(Duration::from_secs(5)),
        ),
    );
    let (result, scripted) = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(caller, target)
    })
    .await?;
    scripted?;
    assert!(
        matches!(
            result,
            Err(ClientError::Local(ref error)) if error.kind() == std::io::ErrorKind::TimedOut
        ),
        "{result:?}"
    );
    Ok(())
}

/// Simulates an overlapped write that delivers bytes to the peer (`inner`) and stays
/// `Poll::Pending` once the total written bytes reach `trigger_at`, until `released` is set.
struct PendingAfterDeliver<W> {
    inner: W,
    trigger_at: usize,
    written: usize,
    pending_len: Option<usize>,
    triggered: bool,
}

impl<W> PendingAfterDeliver<W> {
    const fn new(inner: W, trigger_at: usize) -> Self {
        Self {
            inner,
            trigger_at,
            written: 0,
            pending_len: None,
            triggered: false,
        }
    }
}

impl<W: AsyncWrite + Unpin> AsyncWrite for PendingAfterDeliver<W> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        if self.pending_len.is_some() {
            // Remain pending until the peer's early outcome ends the select.
            return Poll::Pending;
        }
        let poll = Pin::new(&mut self.inner).poll_write(context, buffer);
        if let Poll::Ready(Ok(count)) = poll {
            self.written += count;
            if !self.triggered && self.written >= self.trigger_at {
                self.triggered = true;
                self.pending_len = Some(count);
                return Poll::Pending;
            }
        }
        poll
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(context)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(context)
    }
}

async fn overlapped_stdin_case(
    (trigger_at, expect_end): (usize, bool),
    (close, reply_bytes): (bool, u64),
) -> TestResult<Result<(u64, bool), ClientError>> {
    use ma2a_core::service::{ProcessAccept, ProcessReply};
    use ma2a_services::client::hosted;

    let admitted = ProcessAccept::Admitted.encode()?;
    let ((mut reader, writer), (mut target_reader, mut target_writer)) = pipe();
    let mut writer = FrameWriter::new(PendingAfterDeliver::new(writer.into_inner(), trigger_at));
    let target = async {
        target_writer.frame(FrameKind::Accept, &admitted).await?;
        target_writer.flush().await?;
        assert_eq!(
            target_reader.expect().await?.data()?,
            (Channel::Input, &b"hello"[..])
        );
        if expect_end {
            assert_eq!(target_reader.expect().await?.end()?, Channel::Input);
        }
        let reply = ProcessReply::Written(reply_bytes, close);
        target_writer
            .frame(
                FrameKind::Outcome,
                &ProcessReply::encode_outcome(&Ok(reply))?,
            )
            .await?;
        target_writer.flush().await
    };
    let caller = hosted::stdin((&mut reader, &mut writer), (&b"hello"[..], close));
    let (written, scripted) = within(async { tokio::join!(caller, target) }).await?;
    scripted?;
    Ok(written)
}

async fn overlapped_proxy_case(
    (trigger_at, expect_end): (usize, bool),
    (reason, to_target): (EndReason, u64),
) -> TestResult<Result<ProxyResult, ClientError>> {
    let ((mut reader, writer), (mut target_reader, mut target_writer)) = pipe();
    let mut writer = FrameWriter::new(PendingAfterDeliver::new(writer.into_inner(), trigger_at));
    let target = async {
        target_writer
            .frame(FrameKind::Accept, &EMPTY_ACCEPT)
            .await?;
        target_writer.flush().await?;
        assert_eq!(
            target_reader.expect().await?.data()?,
            (Channel::Input, &b"hello"[..])
        );
        if expect_end {
            assert_eq!(target_reader.expect().await?.end()?, Channel::Input);
            target_writer.end(Channel::Input).await?;
        }
        let outcome = ProxyResult::new(reason, (to_target, 0), 1);
        target_writer
            .frame(
                FrameKind::Outcome,
                &ProxyResult::encode_outcome(&Ok(outcome))?,
            )
            .await?;
        target_writer.flush().await
    };
    let caller = proxy::run(
        (&mut reader, &mut writer),
        (&b"hello"[..], Vec::new()),
        (Duration::from_secs(60), std::future::pending()),
    );
    let (relayed, scripted) = within(async { tokio::join!(caller, target) }).await?;
    scripted?;
    Ok(relayed)
}

#[tokio::test]
async fn overlapped_in_flight_stdin_writes_accept_early_outcomes_and_reject_excess() -> TestResult {
    // 11 bytes = 5 header + 1 channel + 5 payload; 17 bytes = 11 Data + 6 End.
    assert_eq!(
        overlapped_stdin_case((11, false), (true, 5)).await??,
        (5, true)
    );
    assert_eq!(
        overlapped_stdin_case((17, true), (false, 5)).await??,
        (5, false)
    );
    let excess = overlapped_stdin_case((11, false), (true, 6)).await?;
    assert!(malformed(&excess), "{excess:?}");
    Ok(())
}

#[tokio::test]
async fn overlapped_in_flight_proxy_writes_accept_early_outcomes_and_reject_excess() -> TestResult {
    let cancelled = overlapped_proxy_case((11, false), (EndReason::Cancelled, 5)).await??;
    assert_eq!(
        (cancelled.reason, cancelled.bytes_to_target),
        (EndReason::Cancelled, 5)
    );
    let completed = overlapped_proxy_case((17, true), (EndReason::Completed, 5)).await??;
    assert_eq!(
        (completed.reason, completed.bytes_to_target),
        (EndReason::Completed, 5)
    );
    let excess = overlapped_proxy_case((11, false), (EndReason::Cancelled, 6)).await?;
    assert!(miscounted(&excess), "{excess:?}");
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn queued_completed_proxy_and_shell_outcomes_reject_interrupted_delivery() -> TestResult {
    // 1. Proxy with queued Completed outcome after local sink delivery is cancelled.
    let ((mut reader, mut writer), (_target_reader, mut target_writer)) = pipe();
    let (_hold_consumer, local_write) = tokio::io::duplex(1);
    let outcome =
        ProxyResult::encode_outcome(&Ok(ProxyResult::new(EndReason::Completed, (0, 6), 1)))?;
    send(
        &mut target_writer,
        &[
            accept(&EMPTY_ACCEPT),
            Sent::Data(Channel::Input, b"abcdef"),
            Sent::End(Channel::Input),
            Sent::Frame(FrameKind::Outcome, outcome),
        ],
    )
    .await?;
    let relayed = within(proxy::run(
        (&mut reader, &mut writer),
        (&b""[..], local_write),
        (
            Duration::from_secs(60),
            tokio::time::sleep(Duration::from_millis(50)),
        ),
    ))
    .await?;
    assert!(
        matches!(
            relayed,
            Err(ClientError::Local(ref error)) if error.kind() == std::io::ErrorKind::Interrupted
        ),
        "{relayed:?}"
    );

    // 2. Process::run (Shell) with queued Completed outcome after local sink delivery is cancelled.
    let ((mut reader, mut writer), (_target_reader, mut target_writer)) = pipe();
    let (_hold_consumer, out_sink) = tokio::io::duplex(1);
    let completed =
        ProcessResult::new(Termination::Exited(0), EndReason::Completed, 1).with_bytes(6, 0);
    send(
        &mut target_writer,
        &[
            accept(&EMPTY_ACCEPT),
            Sent::Data(Channel::Output, b"abcdef"),
            Sent::End(Channel::Output),
            Sent::Frame(
                FrameKind::Outcome,
                ProcessResult::encode_outcome(&Ok(completed))?,
            ),
        ],
    )
    .await?;
    let ran = within(process::run(
        (&mut reader, &mut writer),
        ProcessIo {
            input: None::<&[u8]>,
            output: out_sink,
            error: None::<Vec<u8>>,
        },
        ProcessControl {
            resizes: None,
            cancel: tokio::time::sleep(Duration::from_millis(50)),
        },
    ))
    .await?;
    assert!(
        matches!(
            ran,
            Err(ClientError::Local(ref error)) if error.kind() == std::io::ErrorKind::Interrupted
        ),
        "{ran:?}"
    );
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn queued_completed_exec_and_service_outcomes_flag_interrupted_delivery() -> TestResult {
    // 1. Process::run_streamed (Exec) with queued Completed outcome flags `delivery_interrupted`.
    let ((mut reader, mut writer), (_target_reader, mut target_writer)) = pipe();
    let (_hold_consumer, out_sink) = tokio::io::duplex(1);
    let completed =
        ProcessResult::new(Termination::Exited(0), EndReason::Completed, 1).with_bytes(6, 0);
    send(
        &mut target_writer,
        &[
            accept(&EMPTY_ACCEPT),
            Sent::Data(Channel::Output, b"abcdef"),
            Sent::End(Channel::Output),
            Sent::Frame(
                FrameKind::Outcome,
                ProcessResult::encode_outcome(&Ok(completed))?,
            ),
        ],
    )
    .await?;
    let streamed = within(process::run_streamed(
        (&mut reader, &mut writer),
        ProcessIo {
            input: None::<&[u8]>,
            output: out_sink,
            error: None::<Vec<u8>>,
        },
        ProcessControl {
            resizes: None,
            cancel: tokio::time::sleep(Duration::from_millis(50)),
        },
    ))
    .await??;
    assert_eq!(streamed.result.reason, EndReason::Completed);
    assert!(streamed.delivery_interrupted);

    // 2. Service::call with queued Ok outcome after interrupted local delivery flags `delivery_interrupted`.
    let ((mut reader, mut writer), (_target_reader, mut target_writer)) = pipe();
    let (_hold_consumer, out_sink) = tokio::io::duplex(1);
    send(
        &mut target_writer,
        &[
            accept(&encode_call_accept(b"h")?),
            Sent::Data(Channel::Output, b"abcdef"),
            Sent::End(Channel::Output),
            Sent::Frame(FrameKind::Outcome, encode_call_outcome(Ok(b"ok"))?),
        ],
    )
    .await?;
    let called = within(service::call(
        (&mut reader, &mut writer),
        ProcessIo {
            input: None::<&[u8]>,
            output: out_sink,
            error: None::<Vec<u8>>,
        },
        tokio::time::sleep(Duration::from_millis(50)),
    ))
    .await??;
    assert_eq!(called.result, b"ok");
    assert!(called.delivery_interrupted);

    // 3. Omitting an unused channel (`error: None`) in `service::call` does not flag
    //    `delivery_interrupted` when only stderr arrives after cancellation.
    let ((mut reader, mut writer), (mut target_reader, mut target_writer)) = pipe();
    let target = async {
        send(&mut target_writer, &[accept(&encode_call_accept(b"h")?)]).await?;
        assert_eq!(target_reader.expect().await?.kind(), FrameKind::Cancel);
        send(
            &mut target_writer,
            &[
                Sent::Data(Channel::Error, b"err"),
                Sent::End(Channel::Error),
                Sent::Frame(FrameKind::Outcome, encode_call_outcome(Ok(b"ok"))?),
            ],
        )
        .await
    };
    let call_task = service::call(
        (&mut reader, &mut writer),
        ProcessIo {
            input: None::<&[u8]>,
            output: Vec::new(),
            error: None::<Vec<u8>>,
        },
        tokio::time::sleep(Duration::from_millis(50)),
    );
    let (res, scripted) = within(async { tokio::join!(call_task, target) }).await?;
    scripted?;
    assert!(!res?.delivery_interrupted);
    Ok(())
}

/// An [`AsyncRead`] that yields a prefix and then fails with `ConnectionReset`.
struct ResetAfter {
    prefix: Vec<u8>,
    offset: usize,
}

impl ResetAfter {
    const fn new(prefix: Vec<u8>) -> Self {
        Self { prefix, offset: 0 }
    }
}

impl tokio::io::AsyncRead for ResetAfter {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        buffer: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if let Some(remaining) = self
            .prefix
            .get(self.offset..)
            .filter(|rest| !rest.is_empty())
        {
            let take = remaining.len().min(buffer.remaining());
            if let Some(chunk) = remaining.get(..take) {
                buffer.put_slice(chunk);
                self.offset += take;
                return Poll::Ready(Ok(()));
            }
        }
        Poll::Ready(Err(std::io::ErrorKind::ConnectionReset.into()))
    }
}

const fn outcome_unknown<T>(result: &Result<T, ClientError>) -> bool {
    matches!(
        result,
        Err(ClientError::Stream(ServiceError::OutcomeUnknown))
    )
}

struct NotifyingBroken(std::sync::Arc<tokio::sync::Notify>);

impl AsyncWrite for NotifyingBroken {
    fn poll_write(
        self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        _bytes: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        self.0.notify_one();
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

async fn stdin_after_broken_write(tail: &[u8]) -> TestResult<Result<(u64, bool), ClientError>> {
    use ma2a_core::service::ProcessAccept;
    use ma2a_services::client::hosted;
    use tokio::io::AsyncWriteExt as _;

    let ((mut reader, _writer), (_target_reader, mut target_writer)) = pipe();
    let wrote = std::sync::Arc::new(tokio::sync::Notify::new());
    let target_wrote = std::sync::Arc::clone(&wrote);
    let tail = tail.to_vec();
    let target = async move {
        let admitted = ProcessAccept::Admitted.encode()?;
        target_writer.frame(FrameKind::Accept, &admitted).await?;
        target_writer.flush().await?;
        target_wrote.notified().await;
        if !tail.is_empty() {
            target_writer.get_mut().write_all(&tail).await?;
        }
        target_writer.shutdown().await?;
        TestResult::Ok(())
    };
    let mut broken = FrameWriter::new(NotifyingBroken(wrote));
    let caller = hosted::stdin((&mut reader, &mut broken), (&b"abc"[..], true));
    let (res, scripted) = within(async { tokio::join!(caller, target) }).await?;
    scripted?;
    Ok(res)
}

async fn assert_streaming_drivers_unknown<R, F>(label: &str, mut make: F) -> TestResult
where
    R: tokio::io::AsyncRead + Unpin,
    F: FnMut(&[u8]) -> R,
{
    use ma2a_core::service::{LogFrom, LogWindow, ProcessAccept, ProcessId, ProcessState};
    use ma2a_services::client::hosted;

    let empty_accept = replies(&[accept(&EMPTY_ACCEPT)]).await?;
    let call_accept = replies(&[accept(&encode_call_accept(b"")?)]).await?;
    let attach_accept = replies(&[accept(
        &ProcessAccept::Reading {
            state: ProcessState::Running,
            start: 0,
            window: LogWindow::new(0, 0),
            truncated: false,
        }
        .encode()?,
    )])
    .await?;

    let res = within(process::run(
        (
            &mut FrameReader::new(make(&empty_accept)),
            &mut FrameWriter::new(tokio::io::sink()),
        ),
        ProcessIo {
            input: None::<&[u8]>,
            output: Vec::new(),
            error: Some(Vec::new()),
        },
        ProcessControl {
            resizes: None,
            cancel: std::future::pending(),
        },
    ))
    .await?;
    assert!(outcome_unknown(&res), "process {label}: {res:?}");

    let res = within(service::call(
        (
            &mut FrameReader::new(make(&call_accept)),
            &mut FrameWriter::new(tokio::io::sink()),
        ),
        ProcessIo {
            input: None::<&[u8]>,
            output: Vec::new(),
            error: Some(Vec::new()),
        },
        std::future::pending(),
    ))
    .await?;
    assert!(outcome_unknown(&res), "service::call {label}: {res:?}");

    let res = within(proxy::run(
        (
            &mut FrameReader::new(make(&empty_accept)),
            &mut FrameWriter::new(tokio::io::sink()),
        ),
        (&b""[..], Vec::new()),
        (Duration::from_secs(60), std::future::pending()),
    ))
    .await?;
    assert!(outcome_unknown(&res), "proxy {label}: {res:?}");

    let res = within(hosted::attach(
        (
            &mut FrameReader::new(make(&attach_accept)),
            &mut FrameWriter::new(tokio::io::sink()),
        ),
        (
            (ProcessId::from_bytes([1; 16]), LogFrom::Now),
            ProcessIo {
                input: None::<&[u8]>,
                output: Vec::new(),
                error: None::<Vec<u8>>,
            },
        ),
        std::future::pending(),
    ))
    .await?;
    assert!(outcome_unknown(&res), "hosted::attach {label}: {res:?}");
    Ok(())
}

async fn assert_request_drivers_unknown<R, F>(label: &str, mut make: F) -> TestResult
where
    R: tokio::io::AsyncRead + Unpin,
    F: FnMut(&[u8]) -> R,
{
    let empty_accept = replies(&[accept(&EMPTY_ACCEPT)]).await?;
    let read_req = FilesRequest::Read(RemotePath::parse("docs:/a")?, 0, 64);
    let stat_req = FilesRequest::Stat(RemotePath::parse("docs:/a")?);

    let res = within(files::run(
        (
            &mut FrameReader::new(make(&empty_accept)),
            &mut FrameWriter::new(tokio::io::sink()),
        ),
        &read_req,
        &[],
    ))
    .await?;
    assert!(outcome_unknown(&res), "files read {label}: {res:?}");

    let res = within(files::run(
        (
            &mut FrameReader::new(make(&[])),
            &mut FrameWriter::new(tokio::io::sink()),
        ),
        &stat_req,
        &[],
    ))
    .await?;
    assert!(outcome_unknown(&res), "files stat {label}: {res:?}");

    let res = within(service::describe(&mut FrameReader::new(make(&[])), None)).await?;
    assert!(outcome_unknown(&res), "service::describe {label}: {res:?}");

    let (mut sink, mut hasher) = (Vec::new(), blake3::Hasher::new());
    let res = within(transfer::download_content(
        &mut FrameReader::new(make(&[])),
        (&mut sink, &mut hasher),
        (ContentIdentity::new(16, 1), 0),
    ))
    .await?;
    assert!(outcome_unknown(&res), "transfer {label}: {res:?}");

    let res = within(agent::call(&mut FrameReader::new(make(&[])))).await?;
    assert!(outcome_unknown(&res), "agent::call {label}: {res:?}");
    Ok(())
}

#[tokio::test]
async fn torn_frames_and_stream_resets_report_outcome_unknown_across_client_drivers() -> TestResult
{
    for (label, tail) in [
        ("truncated payload", &[4_u8, 0, 0, 0, 10, 1, b'a', b'b'][..]),
        ("truncated header", &[4_u8, 0][..]),
    ] {
        let with_tail = |prefix: &[u8]| {
            let mut bytes = prefix.to_vec();
            bytes.extend_from_slice(tail);
            std::io::Cursor::new(bytes)
        };
        assert_streaming_drivers_unknown(label, with_tail).await?;
        assert_request_drivers_unknown(label, with_tail).await?;
    }
    let reset = |prefix: &[u8]| ResetAfter::new(prefix.to_vec());
    assert_streaming_drivers_unknown("connection reset", reset).await?;
    assert_request_drivers_unknown("connection reset", reset).await?;

    // A malformed frame header is still InvalidInput, not OutcomeUnknown.
    let stat_req = FilesRequest::Stat(RemotePath::parse("docs:/a")?);
    let oversized_header: &[u8] = &[4, 0, 0x10, 0, 2];
    let res = within(files::run(
        (
            &mut FrameReader::new(oversized_header),
            &mut FrameWriter::new(tokio::io::sink()),
        ),
        &stat_req,
        &[],
    ))
    .await?;
    assert!(malformed(&res), "oversized header: {res:?}");

    // Failed write in hosted::stdin must preserve the read-side stream error instead of
    // surfacing the write-side Cancelled.
    let res = stdin_after_broken_write(&[]).await?;
    assert!(outcome_unknown(&res), "hosted::stdin clean eof: {res:?}");
    let res = stdin_after_broken_write(&[4_u8, 0, 0, 0, 10, 1, b'a', b'b']).await?;
    assert!(
        outcome_unknown(&res),
        "hosted::stdin truncated payload: {res:?}"
    );
    let res = stdin_after_broken_write(oversized_header).await?;
    assert!(malformed(&res), "hosted::stdin oversized header: {res:?}");
    Ok(())
}
