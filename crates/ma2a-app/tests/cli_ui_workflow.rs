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
    time::{Duration, Instant},
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
        let _shutdown = self.run(&["shutdown"]);
        let _cleanup = fs::remove_dir_all(&self.0);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn ui_open_requires_password_and_launches_the_daemon_loopback_url() -> TestResult {
    use std::os::unix::fs::PermissionsExt as _;

    // Given
    let fixture = Fixture::new()?;
    assert!(!fixture.run(&["ui", "open"])?.status.success());
    let control =
        CurrentUserRuntime::open_at(&fixture.0, Arc::new(FixedClock), WebAuthConfig::default())
            .await?;
    control
        .send(ApiCommand::ui_password_set(Zeroizing::new(
            "secure-process-test-password".to_owned(),
        ))?)
        .await?;
    let tools = fixture.0.join("tools");
    fs::create_dir(&tools)?;
    let capture = fixture.0.join("opened-url");
    let opener = tools.join("xdg-open");
    fs::write(
        &opener,
        format!(
            "#!/bin/sh\nset -eu\nprintf '%s' \"$1\" > \"{}\"\n",
            capture.display()
        ),
    )?;
    fs::set_permissions(&opener, fs::Permissions::from_mode(0o700))?;

    // When
    let output = Command::new(env!("CARGO_BIN_EXE_ma2a"))
        .arg("--state-dir")
        .arg(&fixture.0)
        .args(["ui", "open"])
        .env(
            "PATH",
            format!("{}:{}", tools.display(), std::env::var("PATH")?),
        )
        .output()?;

    // Then
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let url = String::from_utf8(output.stdout)?.trim().to_owned();
    assert!(url.starts_with("http://127.0.0.1:"));
    assert!(!url.contains('@'));
    let deadline = Instant::now() + Duration::from_secs(3);
    while !capture.exists() && Instant::now() < deadline {
        std::thread::yield_now();
    }
    assert_eq!(fs::read_to_string(capture)?, url);
    Ok(())
}
