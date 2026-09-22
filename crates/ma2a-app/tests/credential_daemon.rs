//! Credential commands exercised through a daemon this test owns.

use std::fs::{OpenOptions, TryLockError};

#[path = "support/daemon_fixture.rs"]
mod daemon_fixture;

use daemon_fixture::{DaemonFixture, TestResult};

#[test]
fn sessions_revoke_all_uses_the_running_daemon_control_path() -> TestResult {
    // Given
    let fixture = DaemonFixture::running("credential-daemon")?;

    // When
    let output = fixture.run(&["ui", "revoke-all"])?;

    // Then
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout)?,
        "All Web sessions revoked\n"
    );
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .open(fixture.runtime_dir().join("daemon.lock"))?;
    assert!(matches!(lock.try_lock(), Err(TryLockError::WouldBlock)));
    fixture.shutdown()
}

#[test]
fn singular_session_revoke_all_is_rejected_by_cli() -> TestResult {
    // Given
    let fixture = DaemonFixture::idle("credential-cli")?;

    // When
    let output = fixture.run(&["ui", "session", "revoke-all"])?;

    // Then
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr)?;
    assert_eq!(stderr, "unknown command; run ma2a --help\n");
    fixture.shutdown()
}
