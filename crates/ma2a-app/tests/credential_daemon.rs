//! Credential commands exercised through the autostarted daemon boundary.

use std::{
    error::Error,
    fs::{self, OpenOptions, TryLockError},
    path::PathBuf,
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
};

type TestResult = Result<(), Box<dyn Error + Send + Sync>>;
static NEXT_STATE: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> TestResultValue<Self> {
        let serial = NEXT_STATE.fetch_add(1, Ordering::Relaxed);
        let state_dir = std::env::temp_dir().join(format!(
            "ma2a-credential-daemon-{}-{serial}",
            std::process::id()
        ));
        fs::create_dir(&state_dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&state_dir, fs::Permissions::from_mode(0o700))?;
        }
        Ok(Self(state_dir))
    }

    fn command(&self, arguments: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ma2a"));
        command
            .arg("--state-dir")
            .arg(&self.0)
            .args(arguments)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _shutdown = self.command(&["shutdown"]).output();
        let _cleanup = fs::remove_dir_all(&self.0);
    }
}

type TestResultValue<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[test]
fn sessions_revoke_all_uses_the_autostarted_daemon_control_path() -> TestResult {
    // Given
    let fixture = Fixture::new()?;

    // When
    let output = fixture
        .command(&["ui", "sessions", "revoke-all"])
        .output()?;

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
        .open(fixture.0.join("run-v1/daemon.lock"))?;
    assert!(matches!(lock.try_lock(), Err(TryLockError::WouldBlock)));
    Ok(())
}

#[test]
fn singular_session_revoke_all_is_rejected_by_cli() -> TestResult {
    // Given
    let fixture = Fixture::new()?;

    // When
    let output = fixture.command(&["ui", "session", "revoke-all"]).output()?;

    // Then
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr)?;
    assert_eq!(stderr, "unknown command; run ma2a --help\n");
    Ok(())
}
