//! A pause point that lets a test put another process inside a launch.
//!
//! The race it exposes lives between a launcher deciding that nothing owns the
//! state directory and the daemon it launches trying to become the owner. No
//! amount of timing makes that window reliably reachable, so debug builds can
//! stop here on request: the launcher reports that it has reached the window
//! and waits until told to go on. Release builds contain none of this.

use std::{path::Path, time::Duration};

use super::STARTUP_DEADLINE;

/// Names a directory the launcher signals through; unset, launching never pauses.
const HOLD_VARIABLE: &str = "MA2A_TEST_HOLD_BEFORE_SPAWN";

const POLL: Duration = Duration::from_millis(10);

/// Reports `reached` and waits for `release`, bounded so that a test that never
/// releases the launcher cannot leave it waiting for good.
pub(super) async fn before_spawn() {
    let Some(directory) = std::env::var_os(HOLD_VARIABLE) else {
        return;
    };
    let directory = Path::new(&directory);
    let _signalled = std::fs::write(directory.join("reached"), b"");
    let _released = tokio::time::timeout(STARTUP_DEADLINE, async {
        while !directory.join("release").exists() {
            tokio::time::sleep(POLL).await;
        }
    })
    .await;
}
