//! Caller-side Exec, Shell, and Proxy drivers against the real hosts.
//!
//! Test fixtures run `sh -c` scripts as ordinary typed argv; the host never invokes a shell.
#![cfg(unix)]

#[path = "support/host.rs"]
mod support;

use std::time::Duration;

use ma2a_core::service::{
    EndReason, ExecRequest, ProcessSpec, ProxyHost, ProxyRequest, ServiceError, ShellRequest,
    TerminalSize, Termination,
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
    let caller = client::process::run((&mut reader, &mut writer), local, control);
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
