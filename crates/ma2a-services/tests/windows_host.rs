//! Process stdin on Windows, where a write to a child's pipe finishes on another thread.
#![cfg(windows)]

#[path = "support/host.rs"]
mod support;

use std::time::Duration;

use ma2a_core::service::{
    Channel as DataChannel, FrameKind, LogFrom, ProcessId, ProcessSpec, ProcessStart, ServiceError,
};
use ma2a_services::supervisor::{
    Channel, Credentials, GO_DEADLINE, Launch, SupervisorAddress, SupervisorRequest, launch::ready,
    supervise,
};
use support::{TempRoot, TestResult};

/// A supervisor of `program args` with stdin, in a fresh directory.
fn supervised(
    root: &TempRoot,
    (program, args): (&str, &[&str]),
) -> TestResult<(Launch, tokio::task::JoinHandle<std::io::Result<()>>)> {
    let spec = ProcessSpec::new(
        Some(program.to_owned()),
        args.iter().map(|arg| (*arg).to_owned()).collect(),
        None,
    )?;
    let mut secret = [0; 32];
    getrandom::fill(&mut secret)?;
    let mut nonce = [0_u8; 8];
    getrandom::fill(&mut nonce)?;
    let launch = Launch {
        credentials: Credentials::new(ProcessId::random()?, secret),
        directory: root.path().join("process"),
        address: SupervisorAddress::new(format!("ma2a-test-{:016x}", u64::from_le_bytes(nonce))),
        start: ProcessStart::new(spec, true, 1_048_576)?,
    };
    let task = tokio::spawn(supervise(launch.clone()));
    Ok((launch, task))
}

async fn open(launch: &Launch, request: SupervisorRequest) -> TestResult<Channel> {
    let (reader, mut writer) = ready(&launch.address, &launch.credentials).await?;
    writer.frame(FrameKind::Open, &request.to_bytes()).await?;
    Ok((reader, writer))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stdin_the_child_closed_is_refused_not_reported_written() -> TestResult {
    // Given: a child that closes its stdin, says so, and keeps running.
    let close = "$k = Add-Type -Name K -PassThru -MemberDefinition '\
        [DllImport(\"kernel32.dll\")] public static extern System.IntPtr GetStdHandle(int n); \
        [DllImport(\"kernel32.dll\")] public static extern bool CloseHandle(System.IntPtr h);'; \
        [void]$k::CloseHandle($k::GetStdHandle(-10)); \
        [Console]::Out.WriteLine('closed'); [Console]::Out.Flush(); Start-Sleep 60";
    let root = TempRoot::new("win-stdin-closed")?;
    let (launch, task) = supervised(&root, ("powershell", &["-NoProfile", "-Command", close]))?;
    let (mut go, _go) = open(&launch, SupervisorRequest::Go).await?;
    assert_eq!(go.expect().await?.kind(), FrameKind::Outcome);
    let (mut output, _output) =
        open(&launch, SupervisorRequest::Attach(LogFrom::Start, false)).await?;
    assert_eq!(output.expect().await?.kind(), FrameKind::Accept);
    tokio::time::timeout(Duration::from_secs(60), async {
        let mut seen = Vec::new();
        while !String::from_utf8_lossy(&seen).contains("closed") {
            let frame = output.expect().await?;
            seen.extend_from_slice(frame.data()?.1);
        }
        TestResult::Ok(())
    })
    .await??;

    // When
    let (mut reader, mut writer) = open(&launch, SupervisorRequest::Stdin(false)).await?;
    assert_eq!(reader.expect().await?.kind(), FrameKind::Accept);
    writer.data(DataChannel::Input, b"hello").await?;
    writer.end(DataChannel::Input).await?;
    writer.flush().await?;

    // Then: the write fails rather than counting input that never reached the child.
    let answer = tokio::time::timeout(Duration::from_secs(5), reader.expect()).await??;
    assert_eq!(
        answer.rejection().ok(),
        Some(ServiceError::StdinClosed),
        "{answer:?}"
    );
    drop((output, reader, writer));
    let (mut ended, _ended) = open(&launch, SupervisorRequest::Terminate(0)).await?;
    assert_eq!(ended.expect().await?.kind(), FrameKind::Outcome);
    tokio::time::timeout(GO_DEADLINE + Duration::from_secs(20), task).await???;
    Ok(())
}
