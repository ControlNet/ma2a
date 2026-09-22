//! A private state directory and a running server, for the server tests.

use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::{IpcError, LocalApiServer, ServerExit, TestResult};

static NEXT_STATE: AtomicU64 = AtomicU64::new(0);

pub(super) struct TempState(pub(super) PathBuf);

impl TempState {
    pub(super) fn new() -> TestResult<Self> {
        Self::new_named("shutdown-server")
    }

    pub(super) fn new_named(name: &str) -> TestResult<Self> {
        let serial = NEXT_STATE.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("ma2a-{name}-{}-{serial}", std::process::id()));
        fs::create_dir(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
        }
        Ok(Self(path))
    }
}

impl Drop for TempState {
    fn drop(&mut self) {
        let _cleanup_result = fs::remove_dir_all(&self.0);
    }
}

pub(super) struct LiveServer {
    cancellation: CancellationToken,
    task: Option<JoinHandle<Result<ServerExit, IpcError>>>,
}

impl LiveServer {
    pub(super) fn spawn(server: LocalApiServer) -> Self {
        let cancellation = CancellationToken::new();
        let task = tokio::spawn(server.serve(cancellation.child_token()));
        Self {
            cancellation,
            task: Some(task),
        }
    }

    /// Waits for the server to stop on its own, without cancelling it.
    pub(super) async fn exit(mut self) -> TestResult<ServerExit> {
        let task = self
            .task
            .take()
            .ok_or_else(|| std::io::Error::other("live server task missing"))?;
        Ok(task.await??)
    }

    pub(super) async fn cancel(mut self) -> TestResult<ServerExit> {
        self.cancellation.cancel();
        let task = self
            .task
            .take()
            .ok_or_else(|| std::io::Error::other("live server task missing"))?;
        Ok(task.await??)
    }
}

impl Drop for LiveServer {
    fn drop(&mut self) {
        self.cancellation.cancel();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}
