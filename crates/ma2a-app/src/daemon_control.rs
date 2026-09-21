use std::{
    fs::{File, OpenOptions, TryLockError},
    io,
    path::Path,
    process::{Command, Stdio},
    time::Duration,
};

use ma2a_runtime::ipc::{IpcError, IpcPaths, LocalApiClient};

use crate::AppError;

const STARTUP_DEADLINE: Duration = Duration::from_secs(10);
const MAX_RETRY_MILLIS: u64 = 100;

#[derive(Clone, Copy, Debug)]
pub(crate) enum StartOutcome {
    Started,
    AlreadyRunning,
}

pub(crate) async fn is_running(paths: &IpcPaths) -> Result<bool, AppError> {
    match LocalApiClient::new(paths.clone()).probe().await {
        Ok(()) => Ok(true),
        Err(IpcError::Io(error))
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
            ) =>
        {
            Ok(false)
        }
        Err(error) => Err(error.into()),
    }
}

// This path never creates directories, takes startup locks, or spawns a process.
pub(crate) async fn require(paths: &IpcPaths) -> Result<(), AppError> {
    if is_running(paths).await? {
        Ok(())
    } else {
        Err(AppError::DaemonStopped)
    }
}

pub(crate) async fn start(state_dir: &Path, paths: &IpcPaths) -> Result<StartOutcome, AppError> {
    paths.prepare()?;
    let startup_lock = open_lock(&paths.startup_lock_path())?;
    acquire_startup_lock(&startup_lock).await?;
    start_locked(state_dir, paths).await
}

pub(crate) async fn restart(state_dir: &Path, paths: &IpcPaths) -> Result<(), AppError> {
    paths.prepare()?;
    let startup_lock = open_lock(&paths.startup_lock_path())?;
    acquire_startup_lock(&startup_lock).await?;
    match is_running(paths).await {
        Ok(true) => crate::stop_daemon(paths.clone()).await?,
        Ok(false) => {}
        Err(AppError::Ipc(IpcError::VersionMismatch)) => stop_incompatible(paths).await?,
        Err(error) => return Err(error),
    }
    start_locked(state_dir, paths).await?;
    Ok(())
}

pub(crate) async fn stop(paths: &IpcPaths) -> Result<(), AppError> {
    // Preserve the no-state-creation error for a stopped daemon, but allow the
    // lifecycle-only compatibility path once the startup lock is held.
    match require(paths).await {
        Ok(()) | Err(AppError::Ipc(IpcError::VersionMismatch)) => {}
        Err(error) => return Err(error),
    }
    let startup_lock = open_lock(&paths.startup_lock_path())?;
    acquire_startup_lock(&startup_lock).await?;
    match crate::stop_daemon(paths.clone()).await {
        Err(AppError::Ipc(IpcError::VersionMismatch)) => stop_incompatible(paths).await,
        result => result,
    }
}

async fn stop_incompatible(paths: &IpcPaths) -> Result<(), AppError> {
    eprintln!("daemon protocol is incompatible; stopping the existing daemon");
    LocalApiClient::new(paths.clone())
        .shutdown_compatible()
        .await?;
    wait_until_stopped(paths).await
}

async fn start_locked(state_dir: &Path, paths: &IpcPaths) -> Result<StartOutcome, AppError> {
    match is_running(paths).await {
        Ok(true) => return Ok(StartOutcome::AlreadyRunning),
        Ok(false) => {}
        Err(AppError::Ipc(IpcError::VersionMismatch)) => stop_incompatible(paths).await?,
        Err(error) => return Err(error),
    }
    let daemon_lock = open_lock(&paths.lock_path())?;
    match daemon_lock.try_lock() {
        Ok(()) => {
            paths.remove_stale_endpoint()?;
            // The child must acquire this lock itself. Keep the startup lock
            // held by the caller, but release the daemon lock before spawning.
            drop(daemon_lock);
            spawn_daemon(state_dir)?;
        }
        Err(TryLockError::WouldBlock) => {}
        Err(TryLockError::Error(error)) => return Err(error.into()),
    }
    wait_until_live(&LocalApiClient::new(paths.clone())).await?;
    Ok(StartOutcome::Started)
}

async fn acquire_startup_lock(lock: &File) -> Result<(), AppError> {
    tokio::time::timeout(STARTUP_DEADLINE, async {
        let mut attempt = 0;
        loop {
            match lock.try_lock() {
                Ok(()) => return Ok::<(), AppError>(()),
                Err(TryLockError::WouldBlock) => {
                    tokio::time::sleep(retry_delay(attempt)).await;
                    attempt += 1;
                }
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
    let mut command = Command::new(executable);
    command
        .arg("--state-dir")
        .arg(state_dir)
        .arg("daemon-detached")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    let _child = command.spawn()?;
    Ok(())
}

const fn retry_delay(attempt: u32) -> Duration {
    let shift = if attempt > 4 { 4 } else { attempt };
    let millis = 5_u64 << shift;
    Duration::from_millis(if millis > MAX_RETRY_MILLIS {
        MAX_RETRY_MILLIS
    } else {
        millis
    })
}

pub(crate) async fn wait_until_live(client: &LocalApiClient) -> Result<(), AppError> {
    tokio::time::timeout(STARTUP_DEADLINE, async {
        let mut attempt = 0;
        loop {
            match client.probe().await {
                Ok(()) => return Ok::<(), AppError>(()),
                Err(ma2a_runtime::ipc::IpcError::VersionMismatch) => {
                    return Err(ma2a_runtime::ipc::IpcError::VersionMismatch.into());
                }
                Err(_) => {
                    tokio::time::sleep(retry_delay(attempt)).await;
                    attempt += 1;
                }
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
        let mut attempt = 0;
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
            tokio::time::sleep(retry_delay(attempt)).await;
            attempt += 1;
        }
    })
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "daemon shutdown timed out"))??;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{MAX_RETRY_MILLIS, retry_delay};
    use std::time::Duration;

    #[test]
    fn retry_delay_grows_and_stays_bounded() {
        // Given
        let expected = [5, 10, 20, 40, 80, 80, 80];

        // When
        let observed = (0..7).map(retry_delay).collect::<Vec<_>>();

        // Then
        assert_eq!(observed, expected.map(Duration::from_millis));
        assert!(
            observed
                .into_iter()
                .all(|delay| delay <= Duration::from_millis(MAX_RETRY_MILLIS))
        );
    }
}
