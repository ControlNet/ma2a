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
        .args(["ui", "password", "set", "--password", probe])
        .output()?;

    // Then
    assert_eq!(output.status.code(), Some(2));
    assert_probe_absent(probe, &output.stdout, &output.stderr)?;
    Ok(())
}

#[test]
fn invitation_value_is_rejected_from_argv_without_echoing_it() -> TestResult {
    // Given
    let probe = "argv-probe-marker-two";

    // When
    let output = Command::new(env!("CARGO_BIN_EXE_ma2a"))
        .args(["space", "invite", "redeem", probe])
        .output()?;

    // Then
    assert_eq!(output.status.code(), Some(2));
    assert_probe_absent(probe, &output.stdout, &output.stderr)?;
    Ok(())
}

#[test]
fn invitation_from_stdin_is_not_reflected_on_rejection() -> TestResult {
    // Given
    let probe = "ma2ainvite-invalid-probe-marker";
    let mut child = Command::new(env!("CARGO_BIN_EXE_ma2a"))
        .args(["space", "invite", "redeem", "--stdin"])
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

fn assert_probe_absent(probe: &str, stdout: &[u8], stderr: &[u8]) -> TestResult {
    let stdout = String::from_utf8(stdout.to_vec())?;
    let stderr = String::from_utf8(stderr.to_vec())?;
    assert!(!stdout.contains(probe));
    assert!(!stderr.contains(probe));
    Ok(())
}
