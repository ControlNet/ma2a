//! Exec and Shell between real daemons: typed argv, status, streams, terminal I/O, resize.

use std::time::Duration;

use ma2a_core::service::{OpenBody, ProcessSpec, ShellRequest, TerminalSize, Termination};
use ma2a_runtime::{
    api,
    ipc::{IpcPaths, LocalApiClient, StreamUpgrade},
};
use ma2a_services::{
    FrameReader, FrameWriter,
    client::{
        self,
        process::{ProcessControl, ProcessIo},
    },
};

use super::{
    daemon_fixture::TestResult,
    harness::{Lab, content, refused, succeed},
};

#[test]
fn exec_runs_typed_argv_and_returns_its_streams_and_status() -> TestResult {
    let lab = Lab::start("exec", "1")?;
    let run = lab.remote(
        &["exec"],
        &["--", "sh", "-c", "echo out; echo err >&2; exit 3"],
    )?;
    assert_eq!(run.status.code(), Some(3));
    assert_eq!(run.stdout, b"out\n");
    assert!(String::from_utf8(run.stderr)?.contains("err"));
    // Arguments are argv, never shell text.
    let literal = lab.remote(&["exec"], &["--", "echo", "$HOME;ls"])?;
    succeed(&literal)?;
    assert_eq!(literal.stdout, b"$HOME;ls\n");
    let input = content(2 << 20);
    let echoed = lab.member.piped(
        &["exec", "--endpoint", lab.owner.id(), "--stdin", "--", "cat"],
        &input,
    )?;
    succeed(&echoed)?;
    assert!(echoed.stdout == input, "stdin did not round-trip");
    refused(
        &lab.remote(&["exec"], &["--", "/nonexistent/ma2a-program"])?,
        "spawn_failed",
    )?;
    let timed = lab.remote(&["exec"], &["--timeout", "300ms", "--", "sleep", "10"])?;
    assert_eq!(timed.status.code(), Some(124));
    lab.shutdown()
}

#[test]
fn shell_carries_terminal_io_status_and_resize() -> TestResult {
    let lab = Lab::start("shell", "1")?;
    let session = lab.member.piped(
        &["shell", "--endpoint", lab.owner.id(), "--", "sh"],
        b"echo hi-$((6*7))\nexit 4\n",
    )?;
    assert_eq!(session.status.code(), Some(4));
    assert!(String::from_utf8_lossy(&session.stdout).contains("hi-42"));

    // Resize has no CLI trigger without a terminal, so the same stream plane is driven here
    // through the daemon's local API, exactly as the CLI does.
    let runtime = tokio::runtime::Runtime::new()?;
    let shown = runtime.block_on(resized_session(&lab))?;
    assert!(shown.contains("50 132"), "{shown}");
    lab.shutdown()
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    bytes.iter().fold(String::new(), |mut text, byte| {
        let _written = write!(text, "{byte:02x}");
        text
    })
}

async fn resized_session(lab: &Lab) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let request_id = ma2a_core::RequestId::random().map_err(|_| "random")?;
    let upgrade = api::decode_command(&serde_json::to_vec(&serde_json::json!({
        "version": 1,
        "operation": "stream_open",
        "request_id": hex(request_id.as_bytes()),
        "target_endpoint_id": lab.owner.id(),
        "service": "shell",
    }))?)?;
    let (_, target, _) = upgrade.stream_open().ok_or("upgrade")?;
    let client = LocalApiClient::new(IpcPaths::new(lab.member.state_dir())?);
    let StreamUpgrade::Opened(stream) = client.open_stream(&upgrade).await? else {
        return Err("stream refused".into());
    };
    let (read, write) = tokio::io::split(stream);
    let (mut reader, mut writer) = (FrameReader::new(read), FrameWriter::new(write));
    let spec = ProcessSpec::new(
        Some("sh".to_owned()),
        vec!["-c".to_owned(), "read line; stty size".to_owned()],
        None,
    )?;
    let request = ShellRequest::new(
        spec,
        ("dumb".to_owned(), TerminalSize::new(80, 24)?),
        60_000,
    )?;
    let open = ma2a_core::service::OpenHeader::new(request_id, target, OpenBody::Shell(request))
        .encode()?;
    writer
        .frame(ma2a_core::service::FrameKind::Open, &open)
        .await?;
    writer.flush().await?;
    let (sizes, resizes) = tokio::sync::mpsc::channel(1);
    sizes.send(TerminalSize::new(132, 50)?).await?;
    let (mut typed, input) = tokio::io::duplex(64);
    let typing = async {
        tokio::time::sleep(Duration::from_millis(500)).await;
        tokio::io::AsyncWriteExt::write_all(&mut typed, b"go\n").await
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
    let run = client::process::run((&mut reader, &mut writer), local, control);
    let (result, typed) = tokio::join!(run, typing);
    typed?;
    assert_eq!(result?.termination, Termination::Exited(0));
    Ok(String::from_utf8_lossy(&output).into_owned())
}
