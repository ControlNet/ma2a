//! Process-level coverage for the daemon lifecycle itself.
//!
//! These are the only tests that create real detached daemons, because they are
//! the only tests that are about `start`, `stop` and `restart`. Everything else
//! uses the owned foreground daemon in the shared fixture.

use std::{
    fs::{self, OpenOptions, TryLockError},
    time::{Duration, Instant},
};

use serde_json::Value;

#[path = "support/daemon_fixture.rs"]
mod daemon_fixture;

#[path = "daemon_lifecycle/adversarial.rs"]
mod adversarial;
#[cfg(unix)]
#[path = "daemon_lifecycle/compatibility.rs"]
mod compatibility;
#[path = "daemon_lifecycle/explicit.rs"]
mod explicit;
#[cfg(unix)]
#[path = "daemon_lifecycle/fakes.rs"]
mod fakes;
#[path = "daemon_lifecycle/identity.rs"]
mod identity;
#[path = "daemon_lifecycle/race.rs"]
mod race;

use daemon_fixture::{DaemonFixture, TestResult, TestValue};

/// Reads the record the running daemon wrote about itself.
fn daemon_record(fixture: &DaemonFixture) -> TestValue<Value> {
    Ok(serde_json::from_slice(&fs::read(
        fixture.runtime_dir().join("daemon.json"),
    )?)?)
}

fn status_json(fixture: &DaemonFixture) -> TestValue<Value> {
    fixture.json(&["status", "--json"])
}

/// Reports whether the daemon lock is currently held by something.
fn is_owned(fixture: &DaemonFixture) -> TestValue<bool> {
    let path = fixture.runtime_dir().join("daemon.lock");
    if !path.exists() {
        return Ok(false);
    }
    let lock = OpenOptions::new().read(true).write(true).open(path)?;
    match lock.try_lock() {
        Ok(()) => Ok(false),
        Err(TryLockError::WouldBlock) => Ok(true),
        Err(TryLockError::Error(error)) => Err(error.into()),
    }
}

/// Reports whether a process identifier still names a running MA2A daemon.
#[cfg(target_os = "linux")]
fn daemon_process_is_live(pid: u64) -> bool {
    fs::read(format!("/proc/{pid}/cmdline"))
        .map(|command| String::from_utf8_lossy(&command).contains("ma2a"))
        .unwrap_or(false)
}

#[test]
fn unknown_command_reports_user_facing_error() -> TestResult {
    // Given
    let fixture = DaemonFixture::idle("daemon-lifecycle-unknown")?;

    // When
    let output = fixture.run(&["unknown-command"])?;

    // Then
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr)?;
    assert!(stderr.contains("unknown command; run ma2a --help"));
    assert!(!stderr.contains("Usage("));
    fixture.shutdown()
}

#[test]
fn explicit_start_creates_one_private_daemon() -> TestResult {
    // Given
    let mut fixture = DaemonFixture::idle("daemon-lifecycle-start")?;

    // When
    assert!(fixture.start_detached()?.status.success());
    let response = status_json(&fixture)?;

    // Then
    assert_eq!(
        response.pointer("/result/type").and_then(Value::as_str),
        Some("snapshot")
    );
    assert!(is_owned(&fixture)?, "the started daemon must own its lock");
    let record = daemon_record(&fixture)?;
    assert_eq!(record.get("op").and_then(Value::as_str), Some("running"));
    assert!(
        record
            .get("launch_nonce")
            .and_then(Value::as_str)
            .is_some_and(|nonce| nonce.len() == 32),
        "a launched daemon records the launch it belongs to: {record}"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(
            fs::metadata(fixture.runtime_dir())?.permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(fixture.runtime_dir().join("control.sock"))?
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(fixture.runtime_dir().join("daemon.json"))?
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    fixture.shutdown()
}

#[test]
fn concurrent_start_calls_converge_on_one_daemon() -> TestResult {
    // Given
    let mut fixture = DaemonFixture::idle("daemon-lifecycle-concurrent")?;
    let children = (0..20)
        .map(|_| fixture.command(&["start"]).spawn())
        .collect::<Result<Vec<_>, _>>()?;
    fixture.adopt_detached();

    // When
    let outputs = children
        .into_iter()
        .map(std::process::Child::wait_with_output)
        .collect::<Result<Vec<_>, _>>()?;

    // Then
    for output in &outputs {
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let started = outputs
        .iter()
        .filter(|output| String::from_utf8_lossy(&output.stdout).contains("daemon started"))
        .count();
    assert_eq!(started, 1, "exactly one caller may have launched a daemon");
    assert_eq!(
        status_json(&fixture)?
            .pointer("/result/type")
            .and_then(Value::as_str),
        Some("snapshot")
    );
    assert!(is_owned(&fixture)?);
    identity::assert_live_endpoint_matches_persisted(fixture.state_dir())?;
    fixture.shutdown()
}

#[test]
fn graceful_shutdown_releases_singleton_and_removes_endpoint() -> TestResult {
    // Given
    let mut fixture = DaemonFixture::idle("daemon-lifecycle-stop")?;
    assert!(fixture.start_detached()?.status.success());
    let _status = status_json(&fixture)?;
    #[cfg(target_os = "linux")]
    let pid = daemon_record(&fixture)?
        .get("pid")
        .and_then(Value::as_u64)
        .ok_or("the daemon record carries no process identifier")?;

    // When
    let output = fixture.run(&["stop"])?;

    // Then: stop returns only once the daemon has actually gone.
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8(output.stdout)?, "daemon stopped\n");
    assert!(
        !fixture.runtime_dir().join("control.sock").exists(),
        "the endpoint must be gone when stop returns"
    );
    assert!(
        !fixture.runtime_dir().join("daemon.json").exists(),
        "the daemon record must be gone when stop returns"
    );
    assert!(!is_owned(&fixture)?, "the daemon lock must be free");
    #[cfg(target_os = "linux")]
    assert!(
        !daemon_process_is_live(pid),
        "process {pid} was still running after stop returned"
    );
    fixture.shutdown()
}

/// A stop that has to wait is still a stop that proves the daemon went away.
#[test]
fn stop_returns_only_after_the_daemon_has_released_the_directory() -> TestResult {
    // Given
    let mut fixture = DaemonFixture::idle("daemon-lifecycle-teardown")?;
    assert!(fixture.start_detached()?.status.success());

    // When
    let started = Instant::now();
    let output = fixture.run(&["stop"])?;
    let elapsed = started.elapsed();

    // Then
    assert!(output.status.success());
    assert!(!is_owned(&fixture)?);
    assert!(
        elapsed < Duration::from_secs(60),
        "stop took {elapsed:?}, which is the teardown deadline rather than a teardown"
    );
    fixture.shutdown()
}
