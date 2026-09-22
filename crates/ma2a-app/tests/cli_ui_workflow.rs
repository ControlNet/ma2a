//! Process-level daemon-owned UI workflow coverage.

use std::{
    error::Error,
    fs,
    path::PathBuf,
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use ma2a_runtime::{
    api::Command as ApiCommand,
    current_user::CurrentUserRuntime,
    web::{Clock, WebAuthConfig},
};
use zeroize::Zeroizing;

type TestResult = Result<(), Box<dyn Error + Send + Sync>>;
type TestValue<T> = Result<T, Box<dyn Error + Send + Sync>>;
static NEXT_STATE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
struct FixedClock;

impl Clock for FixedClock {
    fn now_ms(&self) -> i64 {
        1_800_000_000_000
    }
}

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> TestValue<Self> {
        let serial = NEXT_STATE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "ma2a-cli-ui-workflow-{}-{serial}",
            std::process::id()
        ));
        fs::create_dir(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
        }
        Ok(Self(path))
    }

    fn run(&self, arguments: &[&str]) -> Result<std::process::Output, std::io::Error> {
        Command::new(env!("CARGO_BIN_EXE_ma2a"))
            .arg("--state-dir")
            .arg(&self.0)
            .args(arguments)
            .output()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _shutdown = self.run(&["stop"]);
        let _cleanup = fs::remove_dir_all(&self.0);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn ui_start_requires_password_and_returns_the_daemon_loopback_url() -> TestResult {
    // Given
    let fixture = Fixture::new()?;
    let started = fixture.run(&["start"])?;
    assert!(
        started.status.success(),
        "{}",
        String::from_utf8_lossy(&started.stderr)
    );
    assert!(!fixture.run(&["ui", "start", "--json"])?.status.success());
    let control =
        CurrentUserRuntime::open_at(&fixture.0, Arc::new(FixedClock), WebAuthConfig::default())
            .await?;
    control
        .send(ApiCommand::ui_password_set(Zeroizing::new(
            "secure-process-test-password".to_owned(),
        ))?)
        .await?;
    // When
    let output = fixture.run(&["ui", "start", "--json"])?;

    // Then
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let status: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let url = status
        .get("url")
        .and_then(serde_json::Value::as_str)
        .ok_or("missing UI URL")?;
    assert!(url.starts_with("http://127.0.0.1:"));
    assert!(!url.contains('@'));
    let restarted = fixture.run(&["restart"])?;
    assert!(
        restarted.status.success(),
        "{}",
        String::from_utf8_lossy(&restarted.stderr)
    );
    let stopped = fixture.run(&["ui", "status", "--json"])?;
    assert!(stopped.status.success());
    let stopped: serde_json::Value = serde_json::from_slice(&stopped.stdout)?;
    assert_eq!(
        stopped.get("running").and_then(serde_json::Value::as_bool),
        Some(false)
    );
    assert!(stopped.get("url").is_some_and(serde_json::Value::is_null));
    assert!(fixture.run(&["ui", "start"])?.status.success());
    Ok(())
}
