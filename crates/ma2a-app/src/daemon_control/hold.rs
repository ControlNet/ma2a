//! Debug-only rendezvous points for lifecycle transition race tests.
//!
//! A test can hold a transition after classification or proven release, then
//! observe a foreground daemon attempting the same startup lock. No timing
//! guess is needed to reach that ownership window. Release builds omit this.

use std::{path::Path, time::Duration};

use super::STARTUP_DEADLINE;

/// Names a directory the launcher signals through; unset, launching never pauses.
const HOLD_VARIABLE: &str = "MA2A_TEST_HOLD_BEFORE_SPAWN";
const STOP_HOLD_VARIABLE: &str = "MA2A_TEST_HOLD_BEFORE_STOP";
const CONTENDED_VARIABLE: &str = "MA2A_TEST_SIGNAL_STARTUP_LOCK_CONTENDED";

const POLL: Duration = Duration::from_millis(10);

/// Reports `reached` and waits for `release`, bounded so that a test that never
/// releases the launcher cannot leave it waiting for good.
pub(super) async fn before_spawn() {
    wait_at(HOLD_VARIABLE).await;
}

/// Pauses after classification under startup.lock and before a stop request.
pub(super) async fn before_stop() {
    wait_at(STOP_HOLD_VARIABLE).await;
}

/// Proves a competing process attempted startup.lock while it was held.
pub(super) fn startup_lock_contended() {
    let Some(directory) = std::env::var_os(CONTENDED_VARIABLE) else {
        return;
    };
    let _signalled = std::fs::write(Path::new(&directory).join("contended"), b"");
}

async fn wait_at(variable: &str) {
    let Some(directory) = std::env::var_os(variable) else {
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
