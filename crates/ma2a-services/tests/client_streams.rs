//! Caller-side Exec, Shell, and Proxy drivers against the real hosts.
//!
//! Test fixtures run `sh -c` scripts as ordinary typed argv; the host never invokes a shell.
#![cfg(unix)]

#[path = "support/host.rs"]
mod support;

use std::time::Duration;

use ma2a_core::service::{
    EMPTY_ACCEPT, EndReason, ExecRequest, FrameKind, ProcessSpec, ProxyHost, ProxyRequest,
    ServiceError, ShellRequest, TerminalSize, Termination, failed_outcome,
};
use ma2a_services::{
    client::{
        self, ClientError,
        process::{ProcessControl, ProcessIo},
    },
    serve_exec, serve_proxy, serve_shell,
};
use support::{TestResult, content, pipe_with};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::TcpListener,
};
use tokio_util::sync::CancellationToken;

fn script(script: &str) -> TestResult<ProcessSpec> {
    Ok(ProcessSpec::new(
        Some("sh".to_owned()),
        vec!["-c".to_owned(), script.to_owned()],
        Some("/".to_owned()),
    )?)
}

async fn within<T>(future: impl std::future::Future<Output = T>) -> TestResult<T> {
    Ok(tokio::time::timeout(Duration::from_secs(30), future).await?)
}

#[tokio::test]
async fn a_full_duplex_echo_through_tiny_pipes_never_deadlocks() -> TestResult {
    // `cat` echoes while input is still arriving, so input and output are both saturated.
    let input = content(3 << 20);
    let request = ExecRequest::new(script("cat; echo done >&2")?, true, 60_000)?;
    let ((mut reader, mut writer), (mut host_reader, mut host_writer)) = pipe_with(64);
    let shutdown = CancellationToken::new();
    let host = serve_exec(request, &shutdown, (&mut host_reader, &mut host_writer));
    let (mut output, mut error) = (Vec::new(), Vec::new());
    let local = ProcessIo {
        input: Some(input.as_slice()),
        output: &mut output,
        error: Some(&mut error),
    };
    let control = ProcessControl {
        resizes: None,
        cancel: std::future::pending(),
    };
    let caller = client::process::run((&mut reader, &mut writer), local, control);
    let (result, _report) = within(async { tokio::join!(caller, host) }).await?;
    let result = result?;
    assert_eq!(
        (result.termination, result.reason),
        (Termination::Exited(0), EndReason::Completed)
    );
    assert!(output == input, "output differs from input");
    assert_eq!(error, b"done\n");
    Ok(())
}

#[tokio::test]
async fn a_caller_cancel_ends_the_process_and_an_unstartable_one_is_refused() -> TestResult {
    let request = ExecRequest::new(script("sleep 30")?, false, 60_000)?;
    let ((mut reader, mut writer), (mut host_reader, mut host_writer)) = pipe_with(1 << 16);
    let shutdown = CancellationToken::new();
    let host = serve_exec(request, &shutdown, (&mut host_reader, &mut host_writer));
    let local = ProcessIo::<&[u8], _, Vec<u8>> {
        input: None,
        output: Vec::new(),
        error: Some(Vec::new()),
    };
    let control = ProcessControl {
        resizes: None,
        cancel: tokio::time::sleep(Duration::from_millis(200)),
    };
    // Boxed: with its timer, the caller and the host together make a large future.
    let caller = Box::pin(client::process::run(
        (&mut reader, &mut writer),
        local,
        control,
    ));
    let (result, _report) = within(async { tokio::join!(caller, host) }).await?;
    assert_eq!(result?.reason, EndReason::Cancelled);

    let spec = ProcessSpec::new(Some("/nonexistent/ma2a".to_owned()), Vec::new(), None)?;
    let request = ExecRequest::new(spec, false, 1_000)?;
    let ((mut reader, mut writer), (mut host_reader, mut host_writer)) = pipe_with(1 << 16);
    let host = serve_exec(request, &shutdown, (&mut host_reader, &mut host_writer));
    let local = ProcessIo::<&[u8], _, Vec<u8>> {
        input: None,
        output: Vec::new(),
        error: Some(Vec::new()),
    };
    let control = ProcessControl {
        resizes: None,
        cancel: std::future::pending(),
    };
    let caller = client::process::run((&mut reader, &mut writer), local, control);
    let (refused, _report) = within(async { tokio::join!(caller, host) }).await?;
    assert!(matches!(
        refused,
        Err(ClientError::Rejected(ServiceError::SpawnFailed))
    ));
    Ok(())
}

#[tokio::test]
async fn a_shell_receives_input_and_resizes_from_the_caller() -> TestResult {
    let spec = script("read line; stty size; echo got-$line")?;
    let request = ShellRequest::new(
        spec,
        ("dumb".to_owned(), TerminalSize::new(80, 24)?),
        60_000,
    )?;
    let ((mut reader, mut writer), (mut host_reader, mut host_writer)) = pipe_with(1 << 16);
    let shutdown = CancellationToken::new();
    let host = serve_shell(request, &shutdown, (&mut host_reader, &mut host_writer));
    let (sizes, resizes) = tokio::sync::mpsc::channel(1);
    sizes.send(TerminalSize::new(120, 40)?).await?;
    let (mut typed, input) = tokio::io::duplex(64);
    let typing = async {
        // The resize is sent first; input follows once it has had time to arrive.
        tokio::time::sleep(Duration::from_millis(300)).await;
        typed.write_all(b"ok\n").await
    };
    let mut output = Vec::new();
    let local = ProcessIo::<_, _, Vec<u8>> {
        input: Some(input),
        output: &mut output,
        error: None,
    };
    let control = ProcessControl {
        resizes: Some(resizes),
        cancel: std::future::pending(),
    };
    let caller = client::process::run((&mut reader, &mut writer), local, control);
    let (result, typed, _report) = within(async { tokio::join!(caller, typing, host) }).await?;
    typed?;
    assert_eq!(result?.termination, Termination::Exited(0));
    let shown = String::from_utf8_lossy(&output);
    assert!(
        shown.contains("40 120") && shown.contains("got-ok"),
        "{shown}"
    );
    Ok(())
}

#[tokio::test]
async fn a_proxied_connection_is_byte_exact_in_both_directions() -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let destination = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await?;
        let mut received = Vec::new();
        socket.read_to_end(&mut received).await?;
        socket.write_all(&received).await?;
        socket.write_all(b"!").await?;
        socket.shutdown().await?;
        Ok::<Vec<u8>, std::io::Error>(received)
    });
    let request = ProxyRequest::new(ProxyHost::new("127.0.0.1")?, port, 60_000)?;
    let ((mut reader, mut writer), (mut host_reader, mut host_writer)) = pipe_with(256);
    let shutdown = CancellationToken::new();
    let host = serve_proxy(request, &shutdown, (&mut host_reader, &mut host_writer));
    let sent = content(1 << 20);
    let mut returned = Vec::new();
    let caller = client::proxy::run((&mut reader, &mut writer), (sent.as_slice(), &mut returned));
    let (result, _report) = within(async { tokio::join!(caller, host) }).await?;
    let result = result?;
    assert_eq!(destination.await??, sent);
    assert_eq!(returned.len(), sent.len() + 1);
    assert!(returned.starts_with(&sent) && returned.ends_with(b"!"));
    assert_eq!(
        (result.bytes_to_target, result.bytes_from_target),
        (1 << 20, (1 << 20) + 1)
    );
    Ok(())
}

/// A local connection that the other end aborted with a reset.
struct Reset;

impl tokio::io::AsyncRead for Reset {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        _context: &mut std::task::Context<'_>,
        _buffer: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Err(std::io::ErrorKind::ConnectionReset.into()))
    }
}

#[tokio::test]
async fn a_reset_local_connection_cancels_the_proxy_instead_of_ending_its_input() -> TestResult {
    // Given: an accepted proxy whose destination stays silent.
    let ((mut reader, mut writer), (mut target_reader, mut target_writer)) = pipe_with(256);
    target_writer
        .frame(FrameKind::Accept, &EMPTY_ACCEPT)
        .await?;

    // When: the local client aborts its connection.
    let caller = client::proxy::run((&mut reader, &mut writer), (Reset, Vec::new()));
    let target = async {
        let frame = target_reader.expect().await?;
        target_writer
            .frame(
                FrameKind::Outcome,
                &failed_outcome(ServiceError::Cancelled)?,
            )
            .await?;
        TestResult::Ok(frame.kind())
    };
    let (result, seen) = within(async { tokio::join!(caller, target) }).await?;

    // Then: the target is told to cancel at once, not that input ended cleanly.
    assert_eq!(seen?, FrameKind::Cancel);
    assert!(
        matches!(result, Err(ClientError::Failed(ServiceError::Cancelled))),
        "{result:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_replayed_outcome_is_told_apart_from_a_run_that_streamed() -> TestResult {
    // Given: a real run, and the same terminal answered alone, as caller-side replay does.
    let request = ExecRequest::new(script("printf durable-output")?, false, 60_000)?;
    let ((mut reader, mut writer), (mut host_reader, mut host_writer)) = pipe_with(1 << 16);
    let shutdown = CancellationToken::new();
    let host = serve_exec(request, &shutdown, (&mut host_reader, &mut host_writer));
    let mut output = Vec::new();
    let local = ProcessIo {
        input: None::<tokio::io::Empty>,
        output: &mut output,
        error: None::<tokio::io::Sink>,
    };
    let control = ProcessControl {
        resizes: None,
        cancel: std::future::pending(),
    };
    let caller = client::process::run_streamed((&mut reader, &mut writer), local, control);
    let (ran, _report) = within(async { tokio::join!(caller, host) }).await?;
    let ran = ran?;
    let ((mut reader, mut writer), (_host_reader, mut host_writer)) = pipe_with(1 << 16);
    let payload = ma2a_core::service::ProcessResult::encode_outcome(&Ok(ran.result))?;
    host_writer
        .frame(ma2a_core::service::FrameKind::Outcome, &payload)
        .await?;
    host_writer.flush().await?;
    let mut replayed_output = Vec::new();
    let local = ProcessIo {
        input: None::<tokio::io::Empty>,
        output: &mut replayed_output,
        error: None::<tokio::io::Sink>,
    };
    let control = ProcessControl {
        resizes: None,
        cancel: std::future::pending(),
    };

    // When
    let replayed = within(client::process::run_streamed(
        (&mut reader, &mut writer),
        local,
        control,
    ))
    .await??;

    // Then
    assert_eq!(output, b"durable-output");
    assert!(!ran.replayed);
    assert!(replayed.replayed);
    assert!(replayed_output.is_empty());
    assert_eq!(replayed.result, ran.result);
    Ok(())
}

/// Local input that delivers one chunk and then fails.
struct FailsAfter(Option<&'static [u8]>);

impl tokio::io::AsyncRead for FailsAfter {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        _context: &mut std::task::Context<'_>,
        buffer: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(self.0.take().map_or_else(
            || Err(std::io::ErrorKind::BrokenPipe.into()),
            |chunk| {
                buffer.put_slice(chunk);
                Ok(())
            },
        ))
    }
}

/// Runs an Exec whose local input is `input`, sent through `wrap` around the caller's stream,
/// against a scripted target that takes the input `ab` and the frame after it, then reports
/// the process ended with `reason`; returns the answer and that last frame's kind.
async fn exec_scripted<I, W>(
    input: I,
    wrap: impl FnOnce(tokio::io::WriteHalf<tokio::io::DuplexStream>) -> W,
    reason: EndReason,
) -> TestResult<(
    Result<ma2a_core::service::ProcessResult, ClientError>,
    FrameKind,
)>
where
    I: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin + Send,
{
    let ((mut reader, writer), (mut target_reader, mut target_writer)) = pipe_with(1 << 16);
    let mut writer = ma2a_services::FrameWriter::new(wrap(writer.into_inner()));
    let target = async {
        target_writer
            .frame(FrameKind::Accept, &EMPTY_ACCEPT)
            .await?;
        target_writer.flush().await?;
        let first = target_reader.expect().await?;
        assert_eq!(
            first.data()?,
            (ma2a_core::service::Channel::Input, &b"ab"[..])
        );
        let second = target_reader.expect().await?;
        let result = ma2a_core::service::ProcessResult::new(Termination::Exited(0), reason, 1);
        let outcome = ma2a_core::service::ProcessResult::encode_outcome(&Ok(result))?;
        target_writer.frame(FrameKind::Outcome, &outcome).await?;
        target_writer.flush().await?;
        TestResult::Ok(second.kind())
    };
    let local = ProcessIo {
        input: Some(input),
        output: Vec::new(),
        error: Some(Vec::new()),
    };
    let control = ProcessControl {
        resizes: None,
        cancel: std::future::pending(),
    };
    let caller = client::process::run((&mut reader, &mut writer), local, control);
    let (result, seen) = within(async { tokio::join!(caller, target) }).await?;
    Ok((result, seen?))
}

#[tokio::test(start_paused = true)]
async fn the_end_of_input_reaches_the_target_through_a_buffered_stream() -> TestResult {
    // Given: a caller stream that holds writes until flushed.
    // When: local input ends after one chunk.
    let (result, last) =
        exec_scripted(&b"ab"[..], tokio::io::BufWriter::new, EndReason::Completed).await?;

    // Then: the target receives the `end`, so a process waiting for it finishes.
    assert_eq!(last, FrameKind::End);
    assert_eq!(result?.reason, EndReason::Completed);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_failed_local_read_cancels_the_process_and_is_reported() -> TestResult {
    // Given: local input that fails after one chunk.
    // When
    let (result, last) = exec_scripted(
        FailsAfter(Some(b"ab")),
        std::convert::identity,
        EndReason::Cancelled,
    )
    .await?;

    // Then: the process is cancelled rather than told its input ended, and the read failure
    // is the answer.
    assert_eq!(last, FrameKind::Cancel);
    assert!(matches!(result, Err(ClientError::Local(_))), "{result:?}");
    Ok(())
}
