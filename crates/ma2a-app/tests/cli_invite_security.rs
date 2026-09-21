//! Process-level invite input security coverage.

use std::{
    error::Error,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

type TestResult = Result<(), Box<dyn Error + Send + Sync>>;
type TestValue<T> = Result<T, Box<dyn Error + Send + Sync>>;
static NEXT_STATE: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> TestValue<Self> {
        let serial = NEXT_STATE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "ma2a-cli-invite-security-{}-{serial}",
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
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _shutdown = self.run(&["stop"]);
        let _cleanup = fs::remove_dir_all(&self.0);
    }
}

#[cfg(unix)]
#[test]
fn invite_redemption_rejects_insecure_file_inputs() -> TestResult {
    use std::os::unix::fs::PermissionsExt as _;

    // Given
    let fixture = Fixture::new()?;
    let invitation = fixture.0.join("insecure.ticket");
    fs::write(&invitation, "ma2ainvite-invalid")?;
    fs::set_permissions(&invitation, fs::Permissions::from_mode(0o644))?;

    // When
    let output = fixture.run(&[
        "space",
        "invite",
        "redeem",
        "--file",
        path_text(&invitation)?,
    ])?;

    // Then
    assert_eq!(output.status.code(), Some(2));
    Ok(())
}

#[cfg(unix)]
#[test]
fn invite_redemption_rejects_symbolic_link_inputs() -> TestResult {
    use std::os::unix::{fs::PermissionsExt as _, fs::symlink};

    // Given
    let fixture = Fixture::new()?;
    let invitation = fixture.0.join("owned.ticket");
    let link = fixture.0.join("linked.ticket");
    fs::write(&invitation, "ma2ainvite-invalid")?;
    fs::set_permissions(&invitation, fs::Permissions::from_mode(0o600))?;
    symlink(&invitation, &link)?;

    // When
    let output = fixture.run(&["space", "invite", "redeem", "--file", path_text(&link)?])?;

    // Then
    assert_eq!(output.status.code(), Some(2));
    Ok(())
}

#[cfg(target_os = "linux")]
#[test]
fn invite_redemption_rejects_fifo_without_blocking() -> TestResult {
    use rustix::fs::Mode;

    // Given
    let fixture = Fixture::new()?;
    let fifo = fixture.0.join("invite.pipe");
    rustix::fs::mknodat(
        rustix::fs::CWD,
        &fifo,
        rustix::fs::FileType::Fifo,
        Mode::from_raw_mode(0o600),
        0,
    )?;
    // When
    let mut child = Command::new(env!("CARGO_BIN_EXE_ma2a"))
        .arg("--state-dir")
        .arg(&fixture.0)
        .args(["space", "invite", "redeem", "--file", path_text(&fifo)?])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let deadline = Instant::now() + Duration::from_secs(2);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill()?;
            child.wait()?;
            return Err("invite redemption blocked while opening a FIFO".into());
        }
        thread::sleep(Duration::from_millis(10));
    };

    // Then
    assert_eq!(status.code(), Some(2));
    Ok(())
}

fn path_text(path: &Path) -> Result<&str, Box<dyn Error + Send + Sync>> {
    path.to_str().ok_or_else(|| "test path is not UTF-8".into())
}
