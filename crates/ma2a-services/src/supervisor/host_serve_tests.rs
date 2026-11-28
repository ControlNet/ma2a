//! Disconnected Watch and Terminate callers release their supervisor connections promptly.

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use ma2a_core::service::{FrameKind, ProcessId, ProcessSpec, ProcessStart};
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

use super::super::{
    address::SupervisorAddress,
    host::{Lifecycle, Shared},
    log::LogWriter,
    proof::Credentials,
    protocol::SupervisorRequest,
    status::SupervisorStatus,
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

struct TempDirectory(std::path::PathBuf);

impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _cleanup = std::fs::remove_dir_all(&self.0);
    }
}

struct ServerFixture {
    _dir: TempDirectory,
    address: SupervisorAddress,
    shared: Arc<Shared>,
    requests: mpsc::Receiver<Lifecycle>,
    stop: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

impl ServerFixture {
    fn start(label: &str) -> TestResult<Self> {
        let mut nonce = [0_u8; 8];
        getrandom::fill(&mut nonce)?;
        let dir = TempDirectory(
            std::env::temp_dir().join(format!("ma2a-{label}-{:016x}", u64::from_le_bytes(nonce))),
        );
        std::fs::create_dir(&dir.0)?;
        let socket_path = dir.0.join("sup.sock");
        let address = SupervisorAddress::new(socket_path.to_str().ok_or("utf8 path")?.to_owned());
        let listener = address.bind()?;
        let spec = ProcessSpec::new(Some("sleep".to_owned()), Vec::new(), None)?;
        let log = LogWriter::create(&dir.0.join("log"), 1_048_576)?;
        let (tx, requests) = mpsc::channel(16);
        let shared = Arc::new(Shared {
            credentials: Credentials::new(ProcessId::random()?, [9; 32]),
            directory: dir.0.clone(),
            start: ProcessStart::new(spec, false, 1_048_576)?,
            status: watch::Sender::new(SupervisorStatus::Running(42, 1)),
            output: watch::Sender::new(log.window()),
            log: Mutex::new(log),
            stdin: tokio::sync::Mutex::new(None),
            writer: AtomicBool::new(false),
            requests: tx,
            connections: AtomicUsize::new(0),
        });
        let stop = CancellationToken::new();
        let task = tokio::spawn(super::serve(listener, Arc::clone(&shared), stop.clone()));
        Ok(Self {
            _dir: dir,
            address,
            shared,
            requests,
            stop,
            task,
        })
    }

    async fn wait_connections(&self, expected: usize) -> TestResult {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while self.shared.connections.load(Ordering::Acquire) != expected {
            if tokio::time::Instant::now() >= deadline {
                return Err(format!(
                    "expected {expected} connections, still {}",
                    self.shared.connections.load(Ordering::Acquire)
                )
                .into());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        Ok(())
    }

    async fn shutdown(self) -> TestResult {
        self.stop.cancel();
        self.task.await?;
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropped_watch_releases_supervisor_connection_while_child_runs() -> TestResult {
    // Given: a running supervisor with one proven Watch that has already received Running.
    let fixture = ServerFixture::start("watch-drop")?;
    let (mut reader, mut writer) = fixture.address.connect(&fixture.shared.credentials).await?;
    writer
        .frame(FrameKind::Open, &SupervisorRequest::Watch.to_bytes())
        .await?;
    writer.flush().await?;
    assert_eq!(reader.expect().await?.kind(), FrameKind::Accept);
    let running = reader.expect().await?;
    assert_eq!(running.kind(), FrameKind::Control);
    assert_eq!(
        SupervisorStatus::from_bytes(running.payload())?,
        SupervisorStatus::Running(42, 1)
    );
    assert_eq!(fixture.shared.connections.load(Ordering::Acquire), 1);

    // When: the caller drops the Watch connection without any child status change.
    drop((reader, writer));

    // Then: the supervisor releases the connection task promptly while the child stays Running.
    fixture.wait_connections(0).await?;
    fixture.shutdown().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropped_terminate_releases_connection_without_cancelling_termination() -> TestResult {
    // Given: a running supervisor whose lifecycle task has received Terminate(60_000) and is
    // still waiting for the child to exit.
    let mut fixture = ServerFixture::start("term-drop")?;
    let (reader, mut writer) = fixture.address.connect(&fixture.shared.credentials).await?;
    writer
        .frame(
            FrameKind::Open,
            &SupervisorRequest::Terminate(60_000).to_bytes(),
        )
        .await?;
    writer.flush().await?;
    let delivered = tokio::time::timeout(Duration::from_secs(2), fixture.requests.recv())
        .await?
        .ok_or("lifecycle channel closed")?;
    let Lifecycle::Terminate(grace_ms, reply) = delivered else {
        return Err(format!("expected Terminate, got {delivered:?}").into());
    };
    assert_eq!(grace_ms, 60_000);
    assert_eq!(fixture.shared.connections.load(Ordering::Acquire), 1);

    // When: the caller disconnects before the child exits.
    drop((reader, writer));

    // Then: the supervisor releases the connection promptly while lifecycle still holds the
    // delivered termination request.
    fixture.wait_connections(0).await?;
    assert!(reply.is_closed());
    fixture.shutdown().await
}
