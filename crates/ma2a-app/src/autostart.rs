use std::{
    fs::{File, OpenOptions, TryLockError},
    io,
    path::Path,
    process::{Command, Stdio},
    time::Duration,
};

use ma2a_runtime::ipc::{IpcPaths, LocalApiClient};

use crate::AppError;

const STARTUP_DEADLINE: Duration = Duration::from_secs(10);

pub(crate) async fn ensure_daemon(state_dir: &Path, paths: &IpcPaths) -> Result<(), AppError> {
    let client = LocalApiClient::new(paths.clone());
    match client.probe().await {
        Ok(()) => return Ok(()),
        Err(ma2a_runtime::ipc::IpcError::VersionMismatch) => {
            return Err(ma2a_runtime::ipc::IpcError::VersionMismatch.into());
        }
        Err(_) => {}
    }
    paths.prepare()?;
    let startup_lock = open_lock(&paths.startup_lock_path())?;
    acquire_startup_lock(&startup_lock).await?;
    match client.probe().await {
        Ok(()) => return Ok(()),
        Err(ma2a_runtime::ipc::IpcError::VersionMismatch) => {
            return Err(ma2a_runtime::ipc::IpcError::VersionMismatch.into());
        }
        Err(_) => {}
    }
    let daemon_lock = open_lock(&paths.lock_path())?;
    match daemon_lock.try_lock() {
        Ok(()) => {
            paths.remove_stale_endpoint()?;
            spawn_daemon(state_dir)?;
            drop(daemon_lock);
        }
        Err(TryLockError::WouldBlock) => {}
        Err(TryLockError::Error(error)) => return Err(error.into()),
    }
    wait_until_live(&client).await
}

async fn acquire_startup_lock(lock: &File) -> Result<(), AppError> {
    tokio::time::timeout(STARTUP_DEADLINE, async {
        loop {
            match lock.try_lock() {
                Ok(()) => return Ok::<(), AppError>(()),
                Err(TryLockError::WouldBlock) => tokio::task::yield_now().await,
                Err(TryLockError::Error(error)) => return Err(error.into()),
            }
        }
    })
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "daemon startup lock timed out"))??;
    Ok(())
}

pub(crate) fn open_lock(path: &Path) -> Result<File, AppError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        Ok(OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(path)?)
    }
    #[cfg(windows)]
    {
        Ok(OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)?)
    }
}

fn spawn_daemon(state_dir: &Path) -> Result<(), AppError> {
    let executable = std::env::current_exe()?;
    let _child = Command::new(executable)
        .arg("--state-dir")
        .arg(state_dir)
        .arg("daemon")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    Ok(())
}

pub(crate) async fn wait_until_live(client: &LocalApiClient) -> Result<(), AppError> {
    tokio::time::timeout(STARTUP_DEADLINE, async {
        loop {
            match client.probe().await {
                Ok(()) => return Ok::<(), AppError>(()),
                Err(ma2a_runtime::ipc::IpcError::VersionMismatch) => {
                    return Err(ma2a_runtime::ipc::IpcError::VersionMismatch.into());
                }
                Err(_) => tokio::task::yield_now().await,
            }
        }
    })
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "daemon startup timed out"))??;
    Ok(())
}

pub(crate) async fn wait_until_stopped(paths: &IpcPaths) -> Result<(), AppError> {
    let client = LocalApiClient::new(paths.clone());
    tokio::time::timeout(STARTUP_DEADLINE, async {
        loop {
            if !client.is_live().await {
                let lock = open_lock(&paths.lock_path())?;
                match lock.try_lock() {
                    Ok(()) => {
                        paths.remove_stale_endpoint()?;
                        return Ok(());
                    }
                    Err(TryLockError::WouldBlock) => {}
                    Err(TryLockError::Error(error)) => return Err(AppError::Io(error)),
                }
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "daemon shutdown timed out"))??;
    Ok(())
}
