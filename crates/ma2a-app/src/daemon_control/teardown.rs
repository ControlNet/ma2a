//! Proving that a daemon really went away.

use std::{fs::TryLockError, io, time::Duration};

use ma2a_runtime::ipc::{DaemonReport, IpcPaths};

use crate::AppError;

use super::{open_lock, retry_delay};

/// Bounds how long a caller waits for a daemon it has asked to stop.
const TEARDOWN_DEADLINE: Duration = Duration::from_secs(60);

/// Waits until the daemon that owned this state directory has released it.
///
/// Accepting a shutdown request is not teardown. The daemon keeps its lock file
/// open for the whole life of its process and never closes it deliberately, so
/// the operating system is what releases the lock, as part of that process
/// ending. Acquiring the lock here is therefore not an inference about whether
/// the daemon exited — it is the exit, reported by the kernel.
///
/// Once it is released, the endpoint left behind is removed while this caller
/// holds the lock, so the directory is reclaimable rather than merely quiet.
///
/// # Errors
/// Returns a timed-out error naming the departing process when the daemon does
/// not release the directory, which is a failure to stop and never a success.
pub(crate) async fn wait_until_released(
    paths: &IpcPaths,
    departing: Option<&DaemonReport>,
) -> Result<(), AppError> {
    let released = tokio::time::timeout(TEARDOWN_DEADLINE, async {
        let mut attempt = 0;
        loop {
            if reclaim(paths)? {
                return Ok::<(), AppError>(());
            }
            tokio::time::sleep(retry_delay(attempt)).await;
            attempt = attempt.saturating_add(1);
        }
    })
    .await;
    match released {
        Ok(result) => result,
        Err(_elapsed) => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!(
                "the daemon did not release this state directory{}",
                still_running(departing)
            ),
        )
        .into()),
    }
}

/// Takes the daemon lock when it is free and clears what the daemon left behind.
fn reclaim(paths: &IpcPaths) -> Result<bool, AppError> {
    let lock = open_lock(&paths.lock_path())?;
    match lock.try_lock() {
        Ok(()) => {
            paths.remove_stale_endpoint()?;
            drop(lock);
            Ok(true)
        }
        Err(TryLockError::WouldBlock) => Ok(false),
        Err(TryLockError::Error(error)) => Err(error.into()),
    }
}

/// Names the process still holding the directory, when the platform can tell.
fn still_running(departing: Option<&DaemonReport>) -> String {
    match departing.map(|report| (report.incarnation().pid(), report.incarnation().is_live())) {
        Some((pid, Some(true))) => format!("; process {pid} is still running"),
        Some((pid, Some(false))) => {
            format!("; process {pid} has gone but the lock is still held")
        }
        Some((pid, None)) => format!("; it reported process {pid}"),
        None => String::new(),
    }
}
