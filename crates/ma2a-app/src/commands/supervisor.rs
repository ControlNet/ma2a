//! Hidden `__process-supervisor` mode: the detached owner of one durable Process.
//!
//! The daemon starts this mode with the launch record on stdin. The supervisor then leaves
//! the daemon's session, so it and its Process outlive a daemon restart.

use ma2a_services::supervisor::{Launch, detach_session, supervise};
use tokio::io::AsyncReadExt as _;

use crate::AppError;

/// Longest launch record; the start it carries always fits one `open` frame.
const MAX_LAUNCH_BYTES: u64 = 65_536;

pub(crate) async fn run() -> Result<(), AppError> {
    let mut record = Vec::new();
    tokio::io::stdin()
        .take(MAX_LAUNCH_BYTES + 1)
        .read_to_end(&mut record)
        .await?;
    if u64::try_from(record.len()).unwrap_or(u64::MAX) > MAX_LAUNCH_BYTES {
        return Err(AppError::Usage("the launch record exceeds its bound"));
    }
    let launch = Launch::from_bytes(&record)?;
    detach_session()?;
    supervise(launch).await?;
    Ok(())
}
