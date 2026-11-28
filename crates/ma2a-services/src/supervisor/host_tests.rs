use std::sync::{Arc, mpsc as std_mpsc};

use ma2a_core::service::{Channel, FrameKind, LogFrom, MAX_DATA_CHUNK_LEN, ProcessId, ProcessSpec};
use rustix::process::{Resource, Signal, getrlimit, setrlimit};

use super::*;
use crate::{
    FrameReader, FrameWriter,
    supervisor::{
        host_attach::{self, StdinRelay},
        protocol::decode_reading,
    },
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _cleanup = std::fs::remove_dir_all(&self.0);
    }
}

fn test_shared(label: &str, limit: u64) -> TestResult<(TempDir, Arc<Shared>)> {
    let mut nonce = [0_u8; 8];
    getrandom::fill(&mut nonce)?;
    let root = TempDir(std::env::temp_dir().join(format!(
        "ma2a-host-{label}-{:016x}",
        u64::from_le_bytes(nonce)
    )));
    let log = LogWriter::create(&log_directory(&root.0), limit)?;
    let spec = ProcessSpec::new(Some("true".to_owned()), Vec::new(), None)?;
    let (requests, _receiver) = mpsc::channel(1);
    let shared = Arc::new(Shared {
        credentials: Credentials::new(ProcessId::random()?, [7; 32]),
        directory: root.0.clone(),
        start: ProcessStart::new(spec, false, 1_048_576)?,
        status: watch::Sender::new(SupervisorStatus::Running(1, 1)),
        output: watch::Sender::new(log.window()),
        log: Mutex::new(log),
        stdin: tokio::sync::Mutex::new(None),
        writer: AtomicBool::new(false),
        requests,
        connections: AtomicUsize::new(0),
    });
    Ok((root, shared))
}

#[tokio::test(flavor = "current_thread")]
async fn failed_writes_do_not_wake_output_watchers_until_record_or_exit_persists() -> TestResult {
    let (_root, shared) = test_shared("log", 0)?;
    let mut watcher = shared.output.subscribe();
    assert!(!watcher.has_changed()?);

    let _caught = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::from_raw(
        Signal::XFSZ.as_raw(),
    ))?;
    let previous = getrlimit(Resource::Fsize);
    let mut lowered = previous;
    lowered.current = Some(4);
    setrlimit(Resource::Fsize, lowered)?;
    for _ in 0..5 {
        shared.append(Channel::Output, b"aaaa");
    }
    setrlimit(Resource::Fsize, previous)?;

    assert!(!watcher.has_changed()?);
    assert_eq!(*watcher.borrow(), LogWindow::new(0, 0));

    shared.sync_log().await;
    assert!(watcher.has_changed()?);
    assert_eq!(*watcher.borrow_and_update(), LogWindow::new(20, 20));
    Ok(())
}

async fn assert_live_attach_accepts(
    shared: &Shared,
    from: LogFrom,
    expected: (u64, u64, u64),
) -> TestResult {
    let (one, two) = tokio::io::duplex(MAX_DATA_CHUNK_LEN);
    let ((daemon_read, daemon_write), (host_read, host_write)) =
        (tokio::io::split(one), tokio::io::split(two));
    let (mut reader, mut writer) = (
        FrameReader::new(daemon_read),
        FrameWriter::new(daemon_write),
    );
    let (mut host_reader, mut host_writer) =
        (FrameReader::new(host_read), FrameWriter::new(host_write));
    let attached = host_attach::attach(
        (&mut host_reader, &mut host_writer),
        shared,
        (from, StdinRelay::Disabled),
    );
    let daemon = async {
        let accept = reader.expect().await?;
        assert_eq!(accept.kind(), FrameKind::Accept, "{accept:?}");
        assert_eq!(decode_reading(accept.payload())?, (expected, false));
        writer.frame(FrameKind::Cancel, &[]).await?;
        writer.flush().await?;
        let outcome = reader.expect().await?;
        assert_eq!(outcome.kind(), FrameKind::Outcome, "{outcome:?}");
        TestResult::Ok(())
    };
    let (attached, daemon) = tokio::join!(attached, daemon);
    attached?;
    daemon?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_appends_publish_windows_in_write_order_and_keep_live_attach_valid() -> TestResult
{
    let (_root, shared) = test_shared("concurrent-publish", 1_048_576)?;
    let (entered_tx, entered_rx) = std_mpsc::sync_channel::<()>(1);
    let (worker2_done_tx, worker2_done_rx) = std_mpsc::sync_channel::<()>(1);

    let first_shared = Arc::clone(&shared);
    let first = tokio::task::spawn_blocking(move || {
        set_before_publish(Box::new(move |shared_ref| {
            let _ = entered_tx.send(());
            if let Ok(guard) = shared_ref.log.try_lock() {
                drop(guard);
                let _ = worker2_done_rx.recv();
            }
        }));
        first_shared.append(Channel::Output, b"aaaa");
    });

    tokio::task::spawn_blocking(move || entered_rx.recv()).await??;
    let second_shared = Arc::clone(&shared);
    let second = tokio::task::spawn_blocking(move || {
        second_shared.append(Channel::Error, b"bbbb");
        let _ = worker2_done_tx.send(());
    });

    first.await?;
    second.await?;

    assert_eq!(*shared.output.borrow(), LogWindow::new(0, 8));
    assert_live_attach_accepts(&shared, LogFrom::Cursor(8), (8, 0, 8)).await
}

#[tokio::test(flavor = "current_thread")]
async fn failed_write_with_unremovable_empty_segment_keeps_window_valid_and_monotonic() -> TestResult
{
    let (_root, shared) = test_shared("unremovable-empty", 0)?;
    let watcher = shared.output.subscribe();
    assert!(!watcher.has_changed()?);

    let _caught = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::from_raw(
        Signal::XFSZ.as_raw(),
    ))?;
    let previous = getrlimit(Resource::Fsize);
    let mut lowered = previous;
    lowered.current = Some(4);
    setrlimit(Resource::Fsize, lowered)?;
    shared.append(Channel::Output, b"aaaa");
    shared.log.lock().unwrap().blocked_deletes.insert(4);
    shared.append(Channel::Output, b"bbbb");
    setrlimit(Resource::Fsize, previous)?;

    let window = *shared.output.borrow();
    assert!(window.first_available <= window.end, "{window:?}");
    assert_eq!(window, LogWindow::new(0, 0));
    assert!(!watcher.has_changed()?);

    assert_live_attach_accepts(&shared, LogFrom::Start, (0, 0, 0)).await
}
