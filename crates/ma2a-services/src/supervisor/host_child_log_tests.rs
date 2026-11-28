//! Supervisor log failure recovery across live and reopened attachments.

use std::time::Duration;

use ma2a_core::service::{
    Channel, FrameKind, LogFrom, MAX_DATA_CHUNK_LEN, ProcessId, ProcessSpec, ProcessStart,
    Termination, decode_skipped,
};
use tokio::sync::{mpsc, watch};

use super::super::{
    host::Shared,
    host_attach::{self, StdinRelay},
    log::{self, LogWriter},
    proof::Credentials,
    status::{EndCause, SupervisorStatus},
};
use crate::{FrameReader, FrameWriter};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
type ReadEnd = FrameReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>;
type WriteEnd = FrameWriter<tokio::io::WriteHalf<tokio::io::DuplexStream>>;

struct TempDirectory(std::path::PathBuf);

impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _cleanup = std::fs::remove_dir_all(&self.0);
    }
}

fn with_file_size_limit<T>(limit: u64, write: impl FnOnce() -> T) -> TestResult<T> {
    use rustix::process::{Resource, Signal, getrlimit, setrlimit};
    let _caught = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::from_raw(
        Signal::XFSZ.as_raw(),
    ))?;
    let previous = getrlimit(Resource::Fsize);
    let mut lowered = previous;
    lowered.current = Some(limit);
    setrlimit(Resource::Fsize, lowered)?;
    let result = write();
    setrlimit(Resource::Fsize, previous)?;
    Ok(result)
}

async fn read_attachment(reader: &mut ReadEnd) -> TestResult<(u64, Option<u64>, SupervisorStatus)> {
    assert_eq!(reader.expect().await?.kind(), FrameKind::Accept);
    let mut data_bytes = 0_u64;
    let mut skipped = None;
    loop {
        let frame = reader.expect().await?;
        match frame.kind() {
            FrameKind::Data => data_bytes += u64::try_from(frame.data()?.1.len())?,
            FrameKind::Control => skipped = Some(decode_skipped(frame.payload())?),
            FrameKind::Outcome => {
                return Ok((
                    data_bytes,
                    skipped,
                    SupervisorStatus::from_bytes(frame.payload())?,
                ));
            }
            kind => return Err(format!("unexpected {kind:?}").into()),
        }
    }
}

fn duplex_ends() -> (ReadEnd, ReadEnd, WriteEnd) {
    let (one, two) = tokio::io::duplex(MAX_DATA_CHUNK_LEN);
    let ((daemon_read, _), (host_read, host_write)) =
        (tokio::io::split(one), tokio::io::split(two));
    (
        FrameReader::new(daemon_read),
        FrameReader::new(host_read),
        FrameWriter::new(host_write),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_write_seals_segment_so_live_and_reopened_readers_see_skip() -> TestResult {
    // Given: a supervisor and a live attachment reading from start.
    let mut nonce = [0_u8; 8];
    getrandom::fill(&mut nonce)?;
    let directory = TempDirectory(std::env::temp_dir().join(format!(
        "ma2a-failed-skip-{:016x}",
        u64::from_le_bytes(nonce)
    )));
    std::fs::create_dir(&directory.0)?;
    let log_dir = directory.0.join("log");
    let log = LogWriter::create(&log_dir, 1_048_576)?;
    let spec = ProcessSpec::new(Some("true".to_owned()), Vec::new(), None)?;
    let (requests, _receiver) = mpsc::channel(1);
    let shared = std::sync::Arc::new(Shared {
        credentials: Credentials::new(ProcessId::random()?, [7; 32]),
        directory: directory.0.clone(),
        start: ProcessStart::new(spec, false, 1_048_576)?,
        status: watch::Sender::new(SupervisorStatus::Awaiting),
        output: watch::Sender::new(log.window()),
        log: std::sync::Mutex::new(log),
        stdin: tokio::sync::Mutex::new(None),
        writer: std::sync::atomic::AtomicBool::new(false),
        requests,
        connections: std::sync::atomic::AtomicUsize::new(0),
    });
    let (mut reader, mut host_reader, mut host_writer) = duplex_ends();
    let live = host_attach::attach(
        (&mut host_reader, &mut host_writer),
        &shared,
        (LogFrom::Start, StdinRelay::Disabled),
    );
    let reading = tokio::spawn(async move { read_attachment(&mut reader).await });

    // When: a chunk commits, the next chunk hits a short write and rolls back, writing resumes,
    // and the child exits.
    shared.append(Channel::Output, b"result: should ");
    with_file_size_limit(27, || shared.append(Channel::Output, b"never "))?;
    shared.append(Channel::Output, b"commit\n");
    shared.sync_log().await;
    shared
        .record(SupervisorStatus::Ended(
            Termination::Exited(0),
            EndCause::Exited,
            2,
        ))
        .await;
    tokio::time::timeout(Duration::from_secs(5), live).await??;
    let (live_bytes, live_skipped, live_status) =
        tokio::time::timeout(Duration::from_secs(5), reading).await???;

    // Then: the live attachment received the skip before the resumed chunk.
    assert!(live_status.is_final(), "{live_status:?}");
    assert_eq!(live_bytes, 22);
    assert_eq!(live_skipped, Some(21));

    // And: a reopened attachment from cursor 15 also sees the skip and resumed chunk.
    let (mut reopened_reader, mut r_host_reader, mut r_host_writer) = duplex_ends();
    let reopened = host_attach::attach(
        (&mut r_host_reader, &mut r_host_writer),
        &shared,
        (LogFrom::Cursor(15), StdinRelay::Disabled),
    );
    let reopened_read = tokio::spawn(async move { read_attachment(&mut reopened_reader).await });
    tokio::time::timeout(Duration::from_secs(5), reopened).await??;
    let (reopened_bytes, reopened_skipped, reopened_status) =
        tokio::time::timeout(Duration::from_secs(5), reopened_read).await???;
    assert!(reopened_status.is_final(), "{reopened_status:?}");
    assert_eq!(reopened_bytes, 7);
    assert_eq!(reopened_skipped, Some(21));
    assert_eq!(log::window(&log_dir)?.end, 28);
    let after = log::read(&log_dir, 15, usize::MAX)?;
    assert_eq!((after.start, after.truncated, after.next), (21, true, 28));
    assert_eq!(after.records, vec![(Channel::Output, b"commit\n".to_vec())]);
    Ok(())
}
