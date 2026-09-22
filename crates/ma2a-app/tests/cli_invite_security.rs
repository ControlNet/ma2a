//! Process-level invite input security coverage.

use std::{
    error::Error,
    fs,
    path::PathBuf,
    process::{Command, Output, Stdio},
    sync::atomic::{AtomicU64, Ordering},
};

use serde_json::Value;

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

    fn create_space(&self, name: &str) -> TestValue<String> {
        let _started = self.run(&["start"])?;
        let output = self.run(&["space", "create", name, "--json"])?;
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
        let _shutdown = self.run(&["stop"]);
        let _cleanup = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn the_invite_ticket_never_enters_a_runtime_json_envelope() -> TestResult {
    // Given
    let fixture = Fixture::new()?;
    let _space = fixture.create_space("secrets")?;
    let invite = fixture.run(&["space", "invite", "secrets"])?;
    assert!(
        invite.status.success(),
        "{}",
        String::from_utf8_lossy(&invite.stderr)
    );
    let ticket = String::from_utf8(invite.stdout)?.trim().to_owned();
    assert!(ticket.starts_with("ma2ainvite"));

    // When
    let listed = String::from_utf8(fixture.run(&["space", "list", "--json"])?.stdout)?;
    let status = String::from_utf8(fixture.run(&["status", "--json"])?.stdout)?;

    // Then
    assert!(!listed.contains(&ticket), "invite leaked into space list");
    assert!(!status.contains(&ticket), "invite leaked into the snapshot");
    assert!(!String::from_utf8(invite.stderr)?.contains(&ticket));
    Ok(())
}

#[test]
fn the_owner_only_ticket_file_does_not_survive_invite_creation() -> TestResult {
    // Given
    let fixture = Fixture::new()?;
    let _space = fixture.create_space("ephemeral")?;

    // When
    let invite = fixture.run(&["space", "invite", "ephemeral"])?;

    // Then
    assert!(
        invite.status.success(),
        "{}",
        String::from_utf8_lossy(&invite.stderr)
    );
    let residue = fs::read_dir(&fixture.0)?
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(".invite-"))
        .collect::<Vec<_>>();
    assert!(
        residue.is_empty(),
        "ticket residue left behind: {residue:?}"
    );
    Ok(())
}

#[test]
fn piped_acceptance_needs_no_flag_and_reaches_the_runtime() -> TestResult {
    use std::io::Write as _;

    // Given
    let fixture = Fixture::new()?;
    let _space = fixture.create_space("piped")?;
    let mut child = Command::new(env!("CARGO_BIN_EXE_ma2a"))
        .arg("--state-dir")
        .arg(&fixture.0)
        .args(["space", "accept"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .ok_or("missing child stdin")?
        .write_all(b"not-a-valid-ticket\n")?;

    // When
    let output = child.wait_with_output()?;

    // Then
    let stderr = String::from_utf8(output.stderr)?;
    assert!(!output.status.success());
    // Reaching Runtime validation proves stdin was consumed without a flag.
    assert!(stderr.contains("invalid_input"), "{stderr}");
    assert!(!stderr.contains("requires"), "{stderr}");
    Ok(())
}

/// The hidden prompt reads whatever terminal standard input is attached to, so a
/// pseudo-terminal on the child's standard input is enough; no controlling
/// terminal is required and none is assumed under CI.
#[cfg(target_os = "linux")]
#[test]
fn interactive_acceptance_prompts_for_the_ticket_without_echoing_it() -> TestResult {
    use std::{
        ffi::OsStr,
        fs::File,
        io::{Read as _, Write as _},
        thread,
        time::Duration,
    };

    use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};

    // Given
    let fixture = Fixture::new()?;
    let _space = fixture.create_space("interactive")?;
    let controller = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY)?;
    grantpt(&controller)?;
    unlockpt(&controller)?;
    let device = ptsname(&controller, Vec::new())?;
    let device = PathBuf::from(OsStr::new(
        std::str::from_utf8(device.as_bytes()).map_err(|_| "pty name is not UTF-8")?,
    ));
    let terminal = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&device)?;
    let mut child = Command::new(env!("CARGO_BIN_EXE_ma2a"))
        .arg("--state-dir")
        .arg(&fixture.0)
        .args(["space", "accept"])
        .stdin(Stdio::from(terminal.try_clone()?))
        .stdout(Stdio::from(terminal.try_clone()?))
        .stderr(Stdio::from(terminal))
        .spawn()?;

    // When
    let mut writer = File::from(controller.try_clone()?);
    thread::sleep(Duration::from_millis(500));
    writer.write_all(b"not-a-valid-ticket\n")?;
    writer.flush()?;
    let status = child.wait()?;
    let mut transcript = vec![0_u8; 4_096];
    let read = File::from(controller)
        .read(&mut transcript)
        .unwrap_or_default();
    let transcript =
        String::from_utf8_lossy(transcript.get(..read).unwrap_or_default()).into_owned();

    // Then
    assert!(!status.success(), "an invalid ticket must not join a Space");
    assert!(transcript.contains("Invite:"), "{transcript}");
    assert!(
        !transcript.contains("not-a-valid-ticket"),
        "the prompt must not echo the ticket: {transcript}"
    );
    Ok(())
}
