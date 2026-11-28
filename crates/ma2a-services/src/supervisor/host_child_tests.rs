//! The lifecycle's end against requests that were already on their way in.

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize},
    },
    time::Duration,
};

use ma2a_core::service::{
    Channel, FrameKind, LogFrom, MAX_DATA_CHUNK_LEN, ProcessId, ProcessSignal, ProcessSpec,
    ProcessStart, ServiceError, Termination, decode_skipped,
};
use tokio::sync::{mpsc, oneshot, watch};

use super::super::{
    host::{Lifecycle, Shared},
    host_attach::{self, StdinRelay},
    log::{self, LogWriter, MAX_RECORD_LEN, SEGMENT_BYTES},
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

fn shared_fixture(
    label: &str,
    spec: ProcessSpec,
) -> TestResult<(TempDirectory, Arc<Shared>, mpsc::Receiver<Lifecycle>)> {
    let mut nonce = [0_u8; 8];
    getrandom::fill(&mut nonce)?;
    let directory = TempDirectory(
        std::env::temp_dir().join(format!("ma2a-{label}-{:016x}", u64::from_le_bytes(nonce))),
    );
    std::fs::create_dir(&directory.0)?;
    let log = LogWriter::create(&directory.0.join("log"), 1_048_576)?;
    let (requests, receiver) = mpsc::channel(16);
    let shared = Arc::new(Shared {
        credentials: Credentials::new(ProcessId::random()?, [7; 32]),
        directory: directory.0.clone(),
        start: ProcessStart::new(spec, false, 1_048_576)?,
        status: watch::Sender::new(SupervisorStatus::Awaiting),
        output: watch::Sender::new(log.window()),
        log: Mutex::new(log),
        stdin: tokio::sync::Mutex::new(None),
        writer: AtomicBool::new(false),
        requests,
        connections: AtomicUsize::new(0),
    });
    Ok((directory, shared, receiver))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_request_that_won_queue_room_before_the_end_is_refused_not_stranded() -> TestResult {
    // Given: a supervisor waiting for `go`, and a daemon connection that has room in the
    // request queue but has not handed its request over yet, as `Sender::send` does between
    // its two steps.
    let spec = ProcessSpec::new(Some("true".to_owned()), Vec::new(), None)?;
    let (_directory, shared, receiver) = shared_fixture("lifecycle", spec)?;
    let mut lifecycle = tokio::spawn(super::lifecycle(Arc::clone(&shared), receiver));
    let permit = shared.requests.reserve().await?;

    // When: the daemon aborts the start, the lifecycle ends, and only then the request lands.
    let (reply, aborted) = oneshot::channel();
    shared.requests.send(Lifecycle::Abort(reply)).await?;
    assert!(matches!(
        aborted.await?,
        Ok(SupervisorStatus::NeverStarted(_))
    ));
    let _ended = tokio::time::timeout(Duration::from_millis(500), &mut lifecycle).await;
    let (reply, answer) = oneshot::channel::<Result<(), ServiceError>>();
    permit.send(Lifecycle::Signal(ProcessSignal::Terminate, reply));

    // Then: the request is dropped, so the connection answers from the final status instead
    // of waiting for a reply that never comes.
    let answered = tokio::time::timeout(Duration::from_secs(5), answer).await;
    assert!(
        matches!(answered, Ok(Err(_))),
        "a request that reached an ended lifecycle must be dropped, not left unanswered"
    );
    tokio::time::timeout(Duration::from_secs(5), lifecycle).await??;
    Ok(())
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closed_terminate_waiters_are_pruned_before_admitting_another() -> TestResult {
    use std::sync::atomic::Ordering;

    use super::super::status::EndCause;

    // Given: a running child that ignores SIGTERM during a long grace period.
    let spec = ProcessSpec::new(
        Some("sh".to_owned()),
        vec!["-c".to_owned(), "trap '' TERM; exec sleep 60".to_owned()],
        None,
    )?;
    let (_directory, shared, receiver) = shared_fixture("term-prune", spec)?;
    let waiter_count = Arc::new(AtomicUsize::new(0));
    let lifecycle = tokio::spawn(super::TERMINATION_WAITERS.scope(
        Arc::clone(&waiter_count),
        super::lifecycle(Arc::clone(&shared), receiver),
    ));
    let (go_reply, go_answer) = oneshot::channel();
    shared.requests.send(Lifecycle::Go(go_reply)).await?;
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(5), go_answer).await??,
        SupervisorStatus::Running(..)
    ));

    // When: multiple Terminate callers drop their receivers while termination is pending,
    // and one connected caller then requests Terminate.
    for _ in 0..8 {
        let (reply, answer) = oneshot::channel();
        shared
            .requests
            .send(Lifecycle::Terminate(60_000, reply))
            .await?;
        drop(answer);
        let (sync_reply, sync_answer) = oneshot::channel();
        shared.requests.send(Lifecycle::Go(sync_reply)).await?;
        let _ = tokio::time::timeout(Duration::from_secs(5), sync_answer).await??;
    }
    let (live_reply, live_answer) = oneshot::channel();
    shared
        .requests
        .send(Lifecycle::Terminate(60_000, live_reply))
        .await?;
    let (sync_reply, sync_answer) = oneshot::channel();
    shared.requests.send(Lifecycle::Go(sync_reply)).await?;
    let _ = tokio::time::timeout(Duration::from_secs(5), sync_answer).await??;

    // Then: closed waiters were pruned before admitting the connected caller.
    assert_eq!(waiter_count.load(Ordering::Acquire), 1);

    // And: ending the child with grace 0 still delivers the terminal status to the live waiter.
    let (kill_reply, kill_answer) = oneshot::channel();
    shared
        .requests
        .send(Lifecycle::Terminate(0, kill_reply))
        .await?;
    let ended = tokio::time::timeout(Duration::from_secs(5), live_answer).await??;
    assert!(
        matches!(ended, SupervisorStatus::Ended(_, EndCause::Terminated, _)),
        "{ended:?}"
    );
    let _ = tokio::time::timeout(Duration::from_secs(5), kill_answer).await??;
    tokio::time::timeout(Duration::from_secs(5), lifecycle).await??;
    Ok(())
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn child_exit_while_retention_is_blocked_signals_discarded_tail_to_live_and_reopened_readers()
-> TestResult {
    // Given: a running supervisor whose oldest segment cannot be deleted and a live reader.
    let limit = 2 * SEGMENT_BYTES;
    let spec = ProcessSpec::new(Some("true".to_owned()), Vec::new(), None)?;
    let (directory, shared, _receiver) = shared_fixture("blocked-exit", spec)?;
    let log_dir = directory.0.join("log");
    {
        let mut log = LogWriter::create(&log_dir, limit)?;
        log.blocked_deletes.insert(0);
        *shared.log.lock().unwrap() = log;
        shared
            .output
            .send_replace(shared.log.lock().unwrap().window());
    }
    let (mut reader, mut host_reader, mut host_writer) = duplex_ends();
    let live = host_attach::attach(
        (&mut host_reader, &mut host_writer),
        &shared,
        (LogFrom::Start, StdinRelay::Disabled),
    );
    let reading = tokio::spawn(async move { read_attachment(&mut reader).await });

    // When: output overflows the blocked log and the child exits without another append.
    let chunk = vec![b'a'; usize::try_from(MAX_RECORD_LEN)?];
    let mut produced = 0_u64;
    while produced < 5 * SEGMENT_BYTES {
        shared.append(Channel::Output, &chunk);
        produced += u64::try_from(chunk.len())?;
    }
    let stalled_end = shared.output.borrow().end;
    assert!(stalled_end < produced, "excess output was discarded");
    shared.sync_log().await;
    shared
        .record(SupervisorStatus::Ended(
            Termination::Exited(0),
            EndCause::Exited,
            2,
        ))
        .await;
    live.await?;
    let (live_bytes, live_skipped, live_status) = reading.await??;

    // Then: the live attachment saw the retained output, the skipped tail, and final status.
    assert!(live_status.is_final(), "{live_status:?}");
    assert_eq!(live_bytes, stalled_end);
    assert_eq!(live_skipped, Some(produced));

    // And: reopened attach and durable reads both report the discarded tail within disk bounds.
    let (mut reopened_reader, mut r_host_reader, mut r_host_writer) = duplex_ends();
    let reopened = host_attach::attach(
        (&mut r_host_reader, &mut r_host_writer),
        &shared,
        (LogFrom::Cursor(stalled_end), StdinRelay::Disabled),
    );
    let reopened_read = tokio::spawn(async move { read_attachment(&mut reopened_reader).await });
    reopened.await?;
    let (reopened_bytes, reopened_skipped, reopened_status) = reopened_read.await??;
    assert!(reopened_status.is_final(), "{reopened_status:?}");
    assert_eq!(reopened_bytes, 0);
    assert_eq!(reopened_skipped, Some(produced));
    let mut on_disk = 0_u64;
    for entry in std::fs::read_dir(&log_dir)? {
        on_disk += entry?.metadata()?.len();
    }
    assert!(on_disk <= limit + SEGMENT_BYTES, "{on_disk} bytes on disk");
    assert_eq!(log::window(&log_dir)?.end, produced);
    let tail = log::read(&log_dir, stalled_end, usize::MAX)?;
    assert!(tail.truncated);
    assert_eq!((tail.start, tail.next), (produced, produced));
    Ok(())
}
