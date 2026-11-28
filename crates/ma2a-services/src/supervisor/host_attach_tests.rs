//! An attachment whose input the child does not take, under a paused clock.

use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicUsize},
};

use ma2a_core::service::{
    Channel, FrameKind, LogFrom, MAX_DATA_CHUNK_LEN, ProcessId, ProcessSpec, ProcessStart,
    Termination,
};
use tokio::{io::DuplexStream, sync::watch};

use super::super::{
    host::Shared,
    host_stdin::Lease,
    log::LogWriter,
    proof::Credentials,
    status::{EndCause, SupervisorStatus},
};
use crate::{FrameReader, FrameWriter, input::INPUT_STALL};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
type Ends = (
    FrameReader<tokio::io::ReadHalf<DuplexStream>>,
    FrameWriter<tokio::io::WriteHalf<DuplexStream>>,
);

/// Output the child wrote: more than the connection holds.
const OUTPUT: usize = 8 * MAX_DATA_CHUNK_LEN;

/// A running child that never reads its stdin, with [`OUTPUT`] bytes in its log.
struct Child {
    directory: std::path::PathBuf,
    shared: Shared,
    _process: tokio::process::Child,
}

impl Drop for Child {
    fn drop(&mut self) {
        let _cleanup = std::fs::remove_dir_all(&self.directory);
    }
}

fn child() -> TestResult<Child> {
    let mut nonce = [0_u8; 8];
    getrandom::fill(&mut nonce)?;
    let directory =
        std::env::temp_dir().join(format!("ma2a-attach-{:016x}", u64::from_le_bytes(nonce)));
    std::fs::create_dir(&directory)?;
    let mut log = LogWriter::create(&directory.join("log"), 1 << 24)?;
    for _ in 0..OUTPUT / 4_096 {
        log.append(Channel::Output, &[b'o'; 4_096])?;
    }
    let mut child = tokio::process::Command::new("sleep")
        .arg("60")
        .stdin(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let spec = ProcessSpec::new(Some("sleep".to_owned()), Vec::new(), None)?;
    let shared = Shared {
        credentials: Credentials::new(ProcessId::random()?, [7; 32]),
        directory: directory.clone(),
        start: ProcessStart::new(spec, true, 1 << 24)?,
        status: watch::Sender::new(SupervisorStatus::Running(1, 1)),
        output: watch::Sender::new(log.window()),
        log: Mutex::new(log),
        stdin: tokio::sync::Mutex::new(child.stdin.take()),
        writer: AtomicBool::new(false),
        requests: tokio::sync::mpsc::channel(1).0,
        connections: AtomicUsize::new(0),
    };
    Ok(Child {
        directory,
        shared,
        _process: child,
    })
}

/// The daemon's and the supervisor's ends of one attachment, holding less than [`OUTPUT`].
fn connection() -> (Ends, Ends) {
    let (one, two) = tokio::io::duplex(MAX_DATA_CHUNK_LEN);
    let ((daemon_read, daemon_write), (read, write)) =
        (tokio::io::split(one), tokio::io::split(two));
    (
        (
            FrameReader::new(daemon_read),
            FrameWriter::new(daemon_write),
        ),
        (FrameReader::new(read), FrameWriter::new(write)),
    )
}

/// Sends `chunks` of input: the first fills the child's pipe, the second waits, and any more
/// are held behind it.
async fn fill(
    writer: &mut FrameWriter<tokio::io::WriteHalf<DuplexStream>>,
    chunks: usize,
) -> TestResult {
    let chunk = vec![b'x'; MAX_DATA_CHUNK_LEN];
    for _ in 0..chunks {
        writer.data(Channel::Input, &chunk).await?;
    }
    Ok(writer.flush().await?)
}

/// Reads the attachment to its outcome: the output bytes, and the status reported.
async fn read_all(
    reader: &mut FrameReader<tokio::io::ReadHalf<DuplexStream>>,
) -> TestResult<(usize, SupervisorStatus)> {
    let mut output = 0;
    loop {
        let frame = reader.expect().await?;
        match frame.kind() {
            FrameKind::Data => output += frame.data()?.1.len(),
            FrameKind::Outcome => {
                return Ok((output, SupervisorStatus::from_bytes(frame.payload())?));
            }
            kind => return Err(format!("unexpected {kind:?}").into()),
        }
    }
}

#[tokio::test(start_paused = true)]
async fn input_the_child_does_not_take_detaches_after_the_stall_bound() -> TestResult {
    // Given: an attachment writing to a running child that never reads its stdin.
    let child = child()?;
    let ((mut reader, mut writer), (mut host_reader, mut host_writer)) = connection();
    let attached = super::attach(
        (&mut host_reader, &mut host_writer),
        &child.shared,
        (LogFrom::Now, super::StdinRelay::Lenient),
    );
    let daemon = async {
        assert_eq!(reader.expect().await?.kind(), FrameKind::Accept);
        fill(&mut writer, 2).await?;
        // When: nothing more is sent.
        tokio::time::timeout(INPUT_STALL * 3, read_all(&mut reader)).await?
    };
    let (attached, read) = tokio::join!(attached, daemon);

    // Then: the attachment detaches with the child still running, and frees the lease.
    attached?;
    let (_, status) = read?;
    assert!(
        matches!(status, SupervisorStatus::Running(..)),
        "{status:?}"
    );
    assert!(Lease::take(&child.shared).await.is_ok());
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_child_that_ended_while_its_input_stalled_still_delivers_all_its_output() -> TestResult {
    // Given: an attachment from the start whose input the child does not take, and a caller
    // that reads none of the output yet.
    let child = child()?;
    let ((mut reader, mut writer), (mut host_reader, mut host_writer)) = connection();
    let attached = super::attach(
        (&mut host_reader, &mut host_writer),
        &child.shared,
        (LogFrom::Start, super::StdinRelay::Lenient),
    );
    let daemon = async {
        assert_eq!(reader.expect().await?.kind(), FrameKind::Accept);
        fill(&mut writer, 3).await?;
        // When: the child ends, while something it started keeps its stdin, and the input
        // stalls.
        let ended = SupervisorStatus::Ended(Termination::Exited(0), EndCause::Exited, 2);
        child.shared.status.send_replace(ended);
        tokio::time::sleep(INPUT_STALL * 2).await;
        read_all(&mut reader).await
    };
    let (attached, read) = tokio::join!(attached, daemon);

    // Then: its end is reported only with every byte of its output.
    attached?;
    let (output, status) = read?;
    assert!(status.is_final(), "{status:?}");
    assert_eq!(output, OUTPUT);
    Ok(())
}

#[tokio::test]
async fn a_continuous_stdin_flood_never_starves_attached_output() -> TestResult {
    // Given: a running child that closed its stdin immediately, with OUTPUT bytes in its log.
    let child = child()?;
    let mut closed_stdin = tokio::process::Command::new("sh")
        .args(["-c", "exec <&-; exec sleep 60"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    *child.shared.stdin.lock().await = closed_stdin.stdin.take();
    let (one, two) = tokio::io::duplex(1 << 20);
    let ((daemon_read, daemon_write), (read, write)) =
        (tokio::io::split(one), tokio::io::split(two));
    let (mut reader, mut writer) = (
        FrameReader::new(daemon_read),
        FrameWriter::new(daemon_write),
    );
    let (mut host_reader, mut host_writer) = (FrameReader::new(read), FrameWriter::new(write));
    let stop = tokio_util::sync::CancellationToken::new();
    let stop_flood = stop.clone();
    let flood = tokio::spawn(async move {
        let chunk = [b'x'; 64];
        while !stop_flood.is_cancelled() {
            if writer.data(Channel::Input, &chunk).await.is_err() {
                break;
            }
        }
        writer
    });
    let attached = super::attach(
        (&mut host_reader, &mut host_writer),
        &child.shared,
        (LogFrom::Start, super::StdinRelay::Lenient),
    );
    let daemon = async {
        assert_eq!(reader.expect().await?.kind(), FrameKind::Accept);
        let frame =
            tokio::time::timeout(std::time::Duration::from_secs(2), reader.expect()).await??;
        assert_eq!(frame.kind(), FrameKind::Data);
        stop.cancel();
        let mut writer = flood.await?;
        writer.frame(FrameKind::Cancel, &[]).await?;
        writer.flush().await?;
        let _ = read_all(&mut reader).await?;
        TestResult::Ok(())
    };
    let (attached, daemon) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        tokio::join!(attached, daemon)
    })
    .await?;
    let _killed = closed_stdin.kill().await;
    daemon?;
    attached?;
    Ok(())
}
