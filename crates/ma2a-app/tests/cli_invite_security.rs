//! Process-level invite input security coverage.

use std::{fs, path::PathBuf, process::Stdio};

use serde_json::Value;

#[path = "support/daemon_fixture.rs"]
mod daemon_fixture;

use daemon_fixture::{DaemonFixture, TestResult, TestValue};

/// Starts an owned daemon with one Space, ready to issue invites for it.
fn with_space(name: &str) -> TestValue<(DaemonFixture, String)> {
    let fixture = DaemonFixture::running("cli-invite-security")?;
    let output = fixture.run_ok(&["space", "create", name, "--json"])?;
    let response: Value = serde_json::from_slice(&output.stdout)?;
    let space = response
        .pointer("/result/payload/space_id")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or("missing created Space ID")?;
    Ok((fixture, space))
}

#[test]
fn the_invite_ticket_never_enters_a_runtime_json_envelope() -> TestResult {
    // Given
    let (fixture, _space) = with_space("secrets")?;
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
    fixture.shutdown()
}

#[test]
fn the_owner_only_ticket_file_does_not_survive_invite_creation() -> TestResult {
    // Given
    let (fixture, _space) = with_space("ephemeral")?;

    // When
    let invite = fixture.run(&["space", "invite", "ephemeral"])?;

    // Then
    assert!(
        invite.status.success(),
        "{}",
        String::from_utf8_lossy(&invite.stderr)
    );
    let residue = fs::read_dir(fixture.state_dir())?
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(".invite-"))
        .collect::<Vec<_>>();
    assert!(
        residue.is_empty(),
        "ticket residue left behind: {residue:?}"
    );
    fixture.shutdown()
}

#[test]
fn piped_acceptance_needs_no_flag_and_reaches_the_runtime() -> TestResult {
    use std::io::Write as _;

    // Given
    let (fixture, _space) = with_space("piped")?;
    let mut child = fixture
        .command(&["space", "accept"])
        .stdin(Stdio::piped())
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
    fixture.shutdown()
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
    let (fixture, _space) = with_space("interactive")?;
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
    let mut child = fixture
        .command(&["space", "accept"])
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
    fixture.shutdown()
}
