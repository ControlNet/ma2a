//! Deciding what to do about a state directory's daemon, and proving it happened.

use std::{
    fs::{File, OpenOptions},
    io,
    path::Path,
    time::Duration,
};

use ma2a_runtime::ipc::{DaemonReport, IpcError, IpcPaths, LocalApiClient};

use crate::AppError;

#[cfg(debug_assertions)]
mod hold;
mod legacy;
mod spawn;
mod state;
mod teardown;

pub(crate) use spawn::LAUNCH_NONCE_VARIABLE;
use spawn::spawn_daemon;
pub(crate) use state::{DaemonState, classify, endpoint_answers};
pub(crate) use teardown::wait_until_released;

/// Last-resort bound on a daemon that neither reports readiness nor exits.
///
/// Startup does not wait on this: the daemon reports readiness on a pipe, so the
/// common case returns as soon as it is genuinely ready and a daemon that dies is
/// noticed at once through end-of-file. This only stops a caller hanging forever
/// on a child that does neither, so it is deliberately generous; a tight value
/// here failed cold starts that were still legitimately progressing.
const STARTUP_DEADLINE: Duration = Duration::from_secs(60);

const MAX_RETRY_MILLIS: u64 = 100;

#[derive(Clone, Copy, Debug)]
pub(crate) enum StartOutcome {
    Started,
    AlreadyRunning,
}

/// Requires a daemon that can serve business commands.
///
/// This path never creates directories, takes the startup lock, or launches
/// anything. An owner that has stopped answering is reported as exactly that,
/// because telling a caller "not running" would invite it to start a second one.
pub(crate) async fn require(paths: &IpcPaths) -> Result<(), AppError> {
    match classify(paths).await? {
        // Both of these speak this version's business API; only one of them also
        // speaks the lifecycle plane, which business commands do not need.
        DaemonState::Ready(_) | DaemonState::LegacyCurrentApi => Ok(()),
        DaemonState::Absent => Err(AppError::DaemonStopped),
        DaemonState::IncompatibleLifecycle(_) | DaemonState::LegacyIncompatible => {
            Err(IpcError::VersionMismatch.into())
        }
        refusal => Err(refusal.into_refusal()),
    }
}

/// Starts the daemon for this state directory, or reports why it will not.
pub(crate) async fn start(state_dir: &Path, paths: &IpcPaths) -> Result<StartOutcome, AppError> {
    paths.prepare()?;
    let startup_lock = open_lock(&paths.startup_lock_path())?;
    acquire_startup_lock(&startup_lock).await?;
    start_locked(state_dir, paths).await
}

/// Stops the daemon and returns only once it has released the state directory.
pub(crate) async fn stop(paths: &IpcPaths) -> Result<(), AppError> {
    // Classify before creating anything. Stopping a directory no daemon has ever
    // used must not be the thing that brings that directory into existence.
    if matches!(classify(paths).await?, DaemonState::Absent) {
        return Err(AppError::DaemonStopped);
    }
    paths.prepare()?;
    let startup_lock = open_lock(&paths.startup_lock_path())?;
    acquire_startup_lock(&startup_lock).await?;
    stop_locked(paths).await
}

/// Stops to proven absence and then starts. It never does only the second half.
pub(crate) async fn restart(state_dir: &Path, paths: &IpcPaths) -> Result<(), AppError> {
    paths.prepare()?;
    let startup_lock = open_lock(&paths.startup_lock_path())?;
    acquire_startup_lock(&startup_lock).await?;
    match classify(paths).await? {
        DaemonState::Absent => {}
        DaemonState::Ready(_)
        | DaemonState::IncompatibleLifecycle(_)
        | DaemonState::LegacyCurrentApi
        | DaemonState::LegacyIncompatible => {
            stop_locked(paths).await?;
            // Starting is only allowed once the previous owner is proven gone,
            // and the only thing that proves that is looking again.
            match classify(paths).await? {
                DaemonState::Absent => {}
                remaining => return Err(remaining.into_refusal()),
            }
        }
        refusal => return Err(refusal.into_refusal()),
    }
    start_locked(state_dir, paths).await?;
    Ok(())
}

async fn start_locked(state_dir: &Path, paths: &IpcPaths) -> Result<StartOutcome, AppError> {
    match classify(paths).await? {
        DaemonState::Ready(_) | DaemonState::LegacyCurrentApi => {
            return Ok(StartOutcome::AlreadyRunning);
        }
        DaemonState::Absent => {}
        DaemonState::IncompatibleLifecycle(report) => {
            legacy::announce_replacement();
            stop_through_lifecycle(paths, &report).await?;
        }
        DaemonState::LegacyIncompatible => legacy::stop_incompatible(paths).await?,
        refusal => return Err(refusal.into_refusal()),
    }
    // Nothing is unlinked here. `ma2a daemon` does not take the startup lock, so
    // between the classification above and this point a foreground daemon may
    // have claimed ownership and bound the endpoint. Removing it from here would
    // unlink a live daemon's socket and leave a process that still holds the lock
    // but can no longer be addressed. Only the daemon that has actually acquired
    // the lock reclaims an endpoint, which it does in `daemon::reclaim_endpoint`.
    #[cfg(debug_assertions)]
    hold::before_spawn().await;
    let _launched = spawn_daemon(state_dir, paths).await?;
    Ok(StartOutcome::Started)
}

async fn stop_locked(paths: &IpcPaths) -> Result<(), AppError> {
    match classify(paths).await? {
        DaemonState::Absent => Err(AppError::DaemonStopped),
        DaemonState::Ready(report) => stop_through_lifecycle(paths, &report).await,
        DaemonState::IncompatibleLifecycle(report) => {
            legacy::announce_replacement();
            stop_through_lifecycle(paths, &report).await
        }
        // A daemon that predates the lifecycle plane can only be asked through
        // the command it does understand, on the plane its Runtime serves.
        DaemonState::LegacyCurrentApi => legacy::stop_current_api(paths).await,
        DaemonState::LegacyIncompatible => legacy::stop_incompatible(paths).await,
        refusal => Err(refusal.into_refusal()),
    }
}

/// Stops a daemon through the fixed lifecycle plane and proves it went away.
///
/// The request is bounded by the lifecycle deadline and touches nothing the
/// daemon's Runtime owns, so a Runtime that will never answer cannot prevent it.
async fn stop_through_lifecycle(
    paths: &IpcPaths,
    departing: &DaemonReport,
) -> Result<(), AppError> {
    LocalApiClient::new(paths.clone()).request_stop().await?;
    wait_until_released(paths, Some(departing)).await
}

async fn acquire_startup_lock(lock: &File) -> Result<(), AppError> {
    tokio::time::timeout(STARTUP_DEADLINE, async {
        let mut attempt = 0;
        loop {
            match lock.try_lock() {
                Ok(()) => return Ok::<(), AppError>(()),
                Err(std::fs::TryLockError::WouldBlock) => {
                    tokio::time::sleep(retry_delay(attempt)).await;
                    attempt = attempt.saturating_add(1);
                }
                Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
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

const fn retry_delay(attempt: u32) -> Duration {
    let shift = if attempt > 4 { 4 } else { attempt };
    let millis = 5_u64 << shift;
    Duration::from_millis(if millis > MAX_RETRY_MILLIS {
        MAX_RETRY_MILLIS
    } else {
        millis
    })
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
