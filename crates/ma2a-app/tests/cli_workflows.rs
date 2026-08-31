//! Process-level coverage for invite, relay, and daemon-owned UI workflows.

use std::{
    error::Error,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
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
use serde_json::Value;
use zeroize::Zeroizing;

type TestResult = Result<(), Box<dyn Error + Send + Sync>>;
type TestValue<T> = Result<T, Box<dyn Error + Send + Sync>>;
static NEXT_STATE: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);

#[derive(Debug)]
struct FixedClock;

impl Clock for FixedClock {
    fn now_ms(&self) -> i64 {
        1_800_000_000_000
    }
}

impl Fixture {
    fn new() -> TestValue<Self> {
        let serial = NEXT_STATE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "ma2a-cli-workflows-{}-{serial}",
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

    fn run(&self, arguments: &[&str]) -> Result<Output, std::io::Error> {
        Command::new(env!("CARGO_BIN_EXE_ma2a"))
            .arg("--state-dir")
            .arg(&self.0)
            .args(arguments)
            .output()
    }

    fn create_space(&self) -> TestValue<String> {
        let output = self.run(&["space", "create", "--name", "Workflows", "--json"])?;
        assert_success(&output)?;
        let response: Value = serde_json::from_slice(&output.stdout)?;
        response
            .pointer("/result/payload/space_id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| "missing created Space ID".into())
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _shutdown = self.run(&["shutdown"]);
        let _cleanup = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn invite_creation_writes_once_to_an_owner_only_file() -> TestResult {
    // Given
    let fixture = Fixture::new()?;
    let space = fixture.create_space()?;
    let ticket = fixture.0.join("invite.ticket");

    // When
    let created = fixture.run(&[
        "space",
        "invite",
        "create",
        "--space",
        &space,
        "--ttl",
        "30s",
        "--file",
        path_text(&ticket)?,
    ])?;

    // Then
    assert_success(&created)?;
    assert!(!String::from_utf8(created.stdout)?.contains("ma2ainvite"));
    let invitation = fs::read_to_string(&ticket)?;
    assert!(invitation.starts_with("ma2ainvite"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(fs::metadata(&ticket)?.permissions().mode() & 0o777, 0o600);
    }
    let repeated = fixture.run(&[
        "space",
        "invite",
        "create",
        "--space",
        &space,
        "--ttl",
        "30s",
        "--file",
        path_text(&ticket)?,
    ])?;
    assert!(!repeated.status.success());
    assert_eq!(fs::read_to_string(ticket)?, invitation);
    Ok(())
}

#[test]
fn invite_creation_rejects_noninteractive_stdout_and_invalid_ttl() -> TestResult {
    // Given
    let fixture = Fixture::new()?;
    let space = fixture.create_space()?;

    // When
    let stdout = fixture.run(&[
        "space", "invite", "create", "--space", &space, "--ttl", "30s", "--stdout",
    ])?;
    let ttl = fixture.run(&[
        "space",
        "invite",
        "create",
        "--space",
        &space,
        "--ttl",
        "6m",
        "--file",
        path_text(&fixture.0.join("invalid.ticket"))?,
    ])?;

    // Then
    assert_eq!(stdout.status.code(), Some(2));
    assert_eq!(ttl.status.code(), Some(2));
    Ok(())
}

#[test]
fn external_private_and_public_relays_configure_status_and_disable() -> TestResult {
    // Given
    let fixture = Fixture::new()?;
    let space = fixture.create_space()?;

    // When
    let private = fixture.run(&[
        "relay",
        "private",
        "configure",
        "--listen",
        "127.0.0.1:0",
        "--public-url",
        "https://relay.example",
        "--serve-space",
        &space,
        "--external-tls",
    ])?;
    let public = fixture.run(&[
        "relay",
        "public",
        "configure",
        "--url",
        "https://public.example",
    ])?;

    // Then
    assert_success(&private)?;
    assert_success(&public)?;
    let private_status: Value = serde_json::from_slice(
        &fixture
            .run(&["relay", "private", "status", "--json"])?
            .stdout,
    )?;
    assert_eq!(
        private_status.pointer("/result/payload/configured"),
        Some(&Value::Bool(true))
    );
    assert_eq!(
        private_status
            .pointer("/result/payload/mode")
            .and_then(Value::as_str),
        Some("external_termination")
    );
    let public_status: Value = serde_json::from_slice(
        &fixture
            .run(&["relay", "public", "status", "--json"])?
            .stdout,
    )?;
    assert_eq!(
        public_status
            .pointer("/result/payload/url")
            .and_then(Value::as_str),
        Some("https://public.example")
    );
    assert_success(&fixture.run(&["relay", "private", "disable"])?)?;
    assert_success(&fixture.run(&["relay", "public", "disable"])?)?;
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn ui_open_requires_password_and_launches_the_daemon_loopback_url() -> TestResult {
    // Given
    use std::os::unix::fs::PermissionsExt as _;
    let fixture = Fixture::new()?;
    let refused = fixture.run(&["ui", "open"])?;
    assert!(!refused.status.success());
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
    let path = format!("{}:{}", tools.display(), std::env::var("PATH")?);

    // When
    let output = Command::new(env!("CARGO_BIN_EXE_ma2a"))
        .arg("--state-dir")
        .arg(&fixture.0)
        .args(["ui", "open"])
        .env("PATH", path)
        .output()?;

    // Then
    assert_success(&output)?;
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

fn assert_success(output: &Output) -> TestResult {
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).into_owned().into())
    }
}

fn path_text(path: &Path) -> Result<&str, Box<dyn Error + Send + Sync>> {
    path.to_str().ok_or_else(|| "test path is not UTF-8".into())
}
