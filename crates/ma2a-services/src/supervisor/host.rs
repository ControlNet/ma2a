//! The supervisor process: it owns one child, its output, and its status record.

use std::{
    io,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use ma2a_core::service::{LogWindow, ProcessSignal, ProcessStart, ServiceError};
use tokio::{
    process::ChildStdin,
    sync::{mpsc, oneshot, watch},
};
use tokio_util::sync::CancellationToken;

use super::{
    launch::Launch,
    log::LogWriter,
    proof::Credentials,
    status::{self, SupervisorStatus},
};

/// How long connected daemons may keep reading after the child ended.
const LINGER: Duration = Duration::from_secs(5);

/// A request for the lifecycle task, answered once it has been carried out.
#[derive(Debug)]
pub(super) enum Lifecycle {
    Go(oneshot::Sender<SupervisorStatus>),
    Abort(oneshot::Sender<Result<SupervisorStatus, ServiceError>>),
    Signal(ProcessSignal, oneshot::Sender<Result<(), ServiceError>>),
    Terminate(u32, oneshot::Sender<SupervisorStatus>),
}

/// State every connection and the lifecycle task share.
#[derive(Debug)]
pub(super) struct Shared {
    pub(super) credentials: Credentials,
    pub(super) directory: PathBuf,
    pub(super) start: ProcessStart,
    pub(super) status: watch::Sender<SupervisorStatus>,
    pub(super) log: Mutex<LogWriter>,
    pub(super) output: watch::Sender<LogWindow>,
    pub(super) stdin: tokio::sync::Mutex<Option<ChildStdin>>,
    pub(super) writer: AtomicBool,
    pub(super) requests: mpsc::Sender<Lifecycle>,
    pub(super) connections: AtomicUsize,
}

impl Shared {
    /// Returns the output directory.
    pub(super) fn log_directory(&self) -> PathBuf {
        log_directory(&self.directory)
    }

    /// Records a status durably, then publishes it.
    ///
    /// The write and its syncs run on the blocking pool so a slow disk never stalls the
    /// connections that prove ownership to a daemon.
    pub(super) async fn record(&self, next: SupervisorStatus) {
        let directory = self.directory.clone();
        // A status that cannot be written stays without durable evidence; watchers still
        // learn it so a live daemon is never misled.
        let _written = tokio::task::spawn_blocking(move || status::write(&directory, next)).await;
        self.status.send_replace(next);
    }

    fn publish_window(&self, window: LogWindow) {
        self.output.send_if_modified(|current| {
            if *current == window {
                false
            } else {
                *current = window;
                true
            }
        });
    }

    /// Makes all appended output durable and finalizes any trailing retention gap, on the
    /// blocking pool.
    pub(super) async fn sync_log(self: &Arc<Self>) {
        let shared = Arc::clone(self);
        let window = tokio::task::spawn_blocking(move || {
            let mut log = shared.log.lock().unwrap_or_else(PoisonError::into_inner);
            let _finished = log.finish();
            log.window()
        })
        .await;
        if let Ok(window) = window {
            self.publish_window(window);
        }
    }

    /// Appends one output chunk and publishes the new window.
    pub(super) fn append(&self, channel: ma2a_core::service::Channel, bytes: &[u8]) {
        let mut log = self.log.lock().unwrap_or_else(PoisonError::into_inner);
        let appended = log.append(channel, bytes);
        let window = log.window();
        drop(log);
        if appended.is_ok() {
            self.publish_window(window);
        } else {
            // Keep `first_available` current when retention reclaimed an old segment before the
            // write failed, without waking readers for an unpersisted end cursor.
            self.output.send_if_modified(|current| {
                if current.first_available == window.first_available {
                    false
                } else {
                    current.first_available = window.first_available;
                    true
                }
            });
        }
    }

    /// Takes the single stdin writer lease.
    pub(super) fn lease(&self) -> bool {
        self.writer
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    /// Releases the stdin writer lease.
    pub(super) fn release(&self) {
        self.writer.store(false, Ordering::Release);
    }
}

/// Returns the output directory inside a Process directory.
pub fn log_directory(directory: &Path) -> PathBuf {
    directory.join("log")
}

/// Detaches this supervisor from the daemon's session and terminal (Unix).
///
/// # Errors
/// Returns an I/O error when a new session cannot be created.
#[cfg_attr(
    windows,
    expect(clippy::missing_const_for_fn, reason = "Unix creates a session here")
)]
pub fn detach_session() -> io::Result<()> {
    #[cfg(unix)]
    rustix::process::setsid().map_err(io::Error::from)?;
    Ok(())
}

/// Runs one supervisor until its child ended and connected daemons finished reading.
///
/// # Errors
/// Returns an I/O error when the Process directory, output, or socket cannot be created.
pub async fn supervise(launch: Launch) -> io::Result<()> {
    std::fs::create_dir_all(&launch.directory)?;
    // No status record is written yet: its absence means no child was spawned, exactly as
    // `awaiting` would, and the socket becomes reachable without waiting for a disk sync.
    let limit = u64::from(launch.start.log_limit());
    let log = LogWriter::create(&log_directory(&launch.directory), limit)?;
    let listener = launch.address.bind()?;
    let (requests, receiver) = mpsc::channel(16);
    let window = log.window();
    let shared = Arc::new(Shared {
        credentials: launch.credentials,
        directory: launch.directory,
        start: launch.start,
        status: watch::Sender::new(SupervisorStatus::Awaiting),
        log: Mutex::new(log),
        output: watch::Sender::new(window),
        stdin: tokio::sync::Mutex::new(None),
        writer: AtomicBool::new(false),
        requests,
        connections: AtomicUsize::new(0),
    });
    let stop = CancellationToken::new();
    let serving = tokio::spawn(super::host_serve::serve(
        listener,
        Arc::clone(&shared),
        stop.clone(),
    ));
    super::host_child::lifecycle(Arc::clone(&shared), receiver).await;
    let deadline = tokio::time::Instant::now() + LINGER;
    while shared.connections.load(Ordering::Acquire) > 0 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    stop.cancel();
    let _served = serving.await;
    #[cfg(unix)]
    let _removed = std::fs::remove_file(launch.address.as_str());
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use ma2a_core::service::{Channel, ProcessId, ProcessSpec};
    use rustix::process::{Resource, Signal, getrlimit, setrlimit};

    struct TempDir(PathBuf);

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _cleanup = std::fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn failed_writes_do_not_wake_output_watchers_until_record_or_exit_persists()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut nonce = [0_u8; 8];
        getrandom::fill(&mut nonce)?;
        let root = TempDir(
            std::env::temp_dir().join(format!("ma2a-host-log-{:016x}", u64::from_le_bytes(nonce))),
        );
        let log = LogWriter::create(&log_directory(&root.0), 0)?;
        let spec = ProcessSpec::new(Some("true".to_owned()), Vec::new(), None)?;
        let (requests, _receiver) = mpsc::channel(1);
        let shared = Arc::new(Shared {
            credentials: Credentials::new(ProcessId::random()?, [7; 32]),
            directory: root.0.clone(),
            start: ProcessStart::new(spec, false, 1_048_576)?,
            status: watch::Sender::new(SupervisorStatus::Awaiting),
            output: watch::Sender::new(log.window()),
            log: Mutex::new(log),
            stdin: tokio::sync::Mutex::new(None),
            writer: AtomicBool::new(false),
            requests,
            connections: AtomicUsize::new(0),
        });
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
}
