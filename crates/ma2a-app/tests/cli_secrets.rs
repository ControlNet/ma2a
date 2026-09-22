//! Secret-safe CLI argument regression coverage.

use std::{
    error::Error,
    io::Write as _,
    process::{Command, Stdio},
};

type TestResult = Result<(), Box<dyn Error>>;

#[test]
fn password_value_is_rejected_from_argv_without_echoing_it() -> TestResult {
    // Given
    let probe = "argv-probe-marker-one";

    // When
    let output = Command::new(env!("CARGO_BIN_EXE_ma2a"))
        .args(["ui", "init", "--password", probe])
        .output()?;

    // Then
    assert_eq!(output.status.code(), Some(2));
    assert_probe_absent(probe, &output.stdout, &output.stderr)?;
    Ok(())
}

#[test]
fn invitation_value_is_rejected_from_argv_without_echoing_it() -> TestResult {
    // Given
    let probe = "ma2ainvite-argv-probe-marker-two";

    // When
    let output = Command::new(env!("CARGO_BIN_EXE_ma2a"))
        .args(["space", "accept", probe])
        .output()?;

    // Then
    assert_eq!(output.status.code(), Some(2));
    assert_probe_absent(probe, &output.stdout, &output.stderr)?;
    Ok(())
}

#[test]
fn an_invitation_is_rejected_from_argv_on_every_command() -> TestResult {
    // Given
    let probe = "ma2ainvite-argv-probe-marker-anywhere";

    // When
    let invite = Command::new(env!("CARGO_BIN_EXE_ma2a"))
        .args(["space", "invite", probe])
        .output()?;
    let show = Command::new(env!("CARGO_BIN_EXE_ma2a"))
        .args(["space", "show", probe])
        .output()?;

    // Then
    assert_eq!(invite.status.code(), Some(2));
    assert_eq!(show.status.code(), Some(2));
    assert_probe_absent(probe, &invite.stdout, &invite.stderr)?;
    assert_probe_absent(probe, &show.stdout, &show.stderr)?;
    Ok(())
}

#[test]
fn invitation_from_stdin_is_not_reflected_on_rejection() -> TestResult {
    // Given
    let probe = "ma2ainvite-invalid-probe-marker";
    let mut child = Command::new(env!("CARGO_BIN_EXE_ma2a"))
        .args(["space", "accept"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .ok_or("missing child stdin")?
        .write_all(probe.as_bytes())?;

    // When
    let output = child.wait_with_output()?;

    // Then
    assert!(!output.status.success());
    assert_probe_absent(probe, &output.stdout, &output.stderr)?;
    Ok(())
}

#[test]
fn space_accept_help_is_handled_by_clap() -> TestResult {
    // Given / When
    let output = Command::new(env!("CARGO_BIN_EXE_ma2a"))
        .args(["space", "accept", "--help"])
        .output()?;

    // Then
    assert!(output.status.success());
    Ok(())
}

#[test]
fn invitation_value_with_help_is_rejected_without_echoing_it() -> TestResult {
    // Given
    let probe = "argv-probe-marker-with-help";

    // When
    let output = Command::new(env!("CARGO_BIN_EXE_ma2a"))
        .args(["space", "accept", probe, "--help"])
        .output()?;

    // Then
    assert_eq!(output.status.code(), Some(2));
    assert_probe_absent(probe, &output.stdout, &output.stderr)?;
    Ok(())
}

#[test]
fn space_accept_rejects_every_removed_input_flag() -> TestResult {
    // Given / When
    let stdin = Command::new(env!("CARGO_BIN_EXE_ma2a"))
        .args(["space", "accept", "--stdin"])
        .output()?;
    let file = Command::new(env!("CARGO_BIN_EXE_ma2a"))
        .args(["space", "accept", "--file", "safe.ticket"])
        .output()?;

    // Then
    assert_eq!(stdin.status.code(), Some(2));
    assert_eq!(file.status.code(), Some(2));
    Ok(())
}

#[test]
fn space_accept_still_accepts_the_global_state_directory() -> TestResult {
    // Given / When
    let output = Command::new(env!("CARGO_BIN_EXE_ma2a"))
        .args(["space", "accept", "--state-dir", "/nonexistent-ma2a-state"])
        .output()?;

    // Then
    let stderr = String::from_utf8(output.stderr)?;
    assert!(
        !stderr.contains("secret values are not accepted in argv"),
        "{stderr}"
    );
    Ok(())
}

#[test]
fn inline_password_value_is_rejected_without_echoing_it() -> TestResult {
    // Given
    let probe = "argv-probe-marker-inline-password";

    // When
    let output = Command::new(env!("CARGO_BIN_EXE_ma2a"))
        .args(["ui", "init", &format!("--password={probe}")])
        .output()?;

    // Then
    assert_eq!(output.status.code(), Some(2));
    assert_probe_absent(probe, &output.stdout, &output.stderr)?;
    Ok(())
}

#[test]
fn a_dash_prefixed_value_after_space_accept_is_rejected() -> TestResult {
    // Given
    let probe = "--dash-invite-probe-marker";

    // When
    let output = Command::new(env!("CARGO_BIN_EXE_ma2a"))
        .args(["space", "accept", probe])
        .output()?;

    // Then
    assert_eq!(output.status.code(), Some(2));
    assert_probe_absent(probe, &output.stdout, &output.stderr)?;
    Ok(())
}

#[test]
fn dash_prefixed_password_value_is_rejected_without_echoing_it() -> TestResult {
    // Given
    let probe = "--dash-password-probe-marker";

    // When
    let output = Command::new(env!("CARGO_BIN_EXE_ma2a"))
        .args(["ui", "init", probe])
        .output()?;

    // Then
    assert_eq!(output.status.code(), Some(2));
    assert_probe_absent(probe, &output.stdout, &output.stderr)?;
    Ok(())
}

fn assert_probe_absent(probe: &str, stdout: &[u8], stderr: &[u8]) -> TestResult {
    let stdout = String::from_utf8(stdout.to_vec())?;
    let stderr = String::from_utf8(stderr.to_vec())?;
    assert!(!stdout.contains(probe));
    assert!(!stderr.contains(probe));
    Ok(())
}
