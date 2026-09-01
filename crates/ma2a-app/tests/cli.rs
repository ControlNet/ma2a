//! Endpoint-centric command-surface regression coverage.

use std::{
    error::Error,
    fs,
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
};

type TestResult = Result<(), Box<dyn Error>>;
type TestValue<T> = Result<T, Box<dyn Error>>;
static NEXT_STATE: AtomicU64 = AtomicU64::new(0);

struct TempState(PathBuf);

impl TempState {
    fn new() -> TestValue<Self> {
        let serial = NEXT_STATE.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("ma2a-cli-echo-{}-{serial}", std::process::id()));
        fs::create_dir(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
        }
        Ok(Self(path))
    }
}

impl Drop for TempState {
    fn drop(&mut self) {
        let _shutdown = Command::new(env!("CARGO_BIN_EXE_ma2a"))
            .args([
                "--state-dir",
                self.0.to_str().unwrap_or_default(),
                "shutdown",
            ])
            .output();
        let _cleanup = fs::remove_dir_all(&self.0);
    }
}

fn ma2a() -> Command {
    Command::new(env!("CARGO_BIN_EXE_ma2a"))
}

#[test]
fn help_lists_endpoint_centric_workflows() -> TestResult {
    // Given
    let mut command = ma2a();

    // When
    let output = command.arg("--help").output()?;

    // Then
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout)?;
    for workflow in ["endpoint", "space", "relay", "echo", "ui"] {
        assert!(
            stdout.contains(workflow),
            "missing {workflow} in help:\n{stdout}"
        );
    }
    assert!(
        !stdout.contains("web"),
        "unexpected web command in help:\n{stdout}"
    );
    Ok(())
}

#[test]
fn ui_help_lists_open_workflow() -> TestResult {
    // Given
    let mut command = ma2a();

    // When
    let output = command.args(["ui", "--help"]).output()?;

    // Then
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout)?;
    assert!(
        stdout.contains("open"),
        "missing open in UI help:\n{stdout}"
    );
    Ok(())
}

#[test]
fn echo_rejects_space_addressing() -> TestResult {
    // Given
    let mut command = ma2a();

    // When
    let output = command
        .args([
            "echo",
            "--endpoint",
            "0000000000000000000000000000000000000000000000000000000000000000",
            "--via-space",
            "0000000000000000000000000000000000000000000000000000000000000000",
            "--text",
            "hello",
        ])
        .output()?;

    // Then
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8(output.stderr)?.contains("--via-space"));
    Ok(())
}

#[test]
fn echo_accepts_endpoint_text_json() -> TestResult {
    // Given
    let state = TempState::new()?;
    let mut command = ma2a();

    // When
    let output = command
        .args([
            "--state-dir",
            state.0.to_str().ok_or("state path is not UTF-8")?,
            "echo",
            "--endpoint",
            "0000000000000000000000000000000000000000000000000000000000000000",
            "--text",
            "hello",
            "--json",
        ])
        .output()?;

    // Then
    assert_ne!(output.status.code(), Some(2));
    assert!(!String::from_utf8(output.stderr)?.contains("cannot be used with"));
    Ok(())
}

#[test]
fn echo_accepts_endpoint_stdin_json() -> TestResult {
    // Given
    let state = TempState::new()?;
    let mut command = ma2a();
    let mut child = command
        .args([
            "--state-dir",
            state.0.to_str().ok_or("state path is not UTF-8")?,
            "echo",
            "--endpoint",
            "0000000000000000000000000000000000000000000000000000000000000000",
            "--stdin",
            "--json",
        ])
        .stdin(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .ok_or("child stdin unavailable")?
        .write_all(b"hello")?;

    // When
    let output = child.wait_with_output()?;

    // Then
    assert_ne!(output.status.code(), Some(2));
    assert!(!String::from_utf8(output.stderr)?.contains("cannot be used with"));
    Ok(())
}

#[test]
fn echo_accepts_endpoint_stdin() -> TestResult {
    // Given
    let state = TempState::new()?;
    let mut command = ma2a();
    let mut child = command
        .args([
            "--state-dir",
            state.0.to_str().ok_or("state path is not UTF-8")?,
            "echo",
            "--endpoint",
            "0000000000000000000000000000000000000000000000000000000000000000",
            "--stdin",
        ])
        .stdin(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .ok_or("child stdin unavailable")?
        .write_all(b"hello")?;

    // When
    let output = child.wait_with_output()?;

    // Then
    assert_ne!(output.status.code(), Some(2));
    assert!(!String::from_utf8(output.stderr)?.contains("cannot be used with"));
    Ok(())
}

#[test]
fn echo_rejects_dual_payload_sources() -> TestResult {
    // Given
    let mut command = ma2a();

    // When
    let output = command
        .args([
            "echo",
            "--endpoint",
            "0000000000000000000000000000000000000000000000000000000000000000",
            "--text",
            "hello",
            "--stdin",
        ])
        .output()?;

    // Then
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr)?;
    assert!(stderr.contains("--text") || stderr.contains("--stdin"));
    Ok(())
}

#[test]
fn echo_rejects_missing_endpoint() -> TestResult {
    // Given
    let mut command = ma2a();

    // When
    let output = command.args(["echo", "--text", "hello"]).output()?;

    // Then
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8(output.stderr)?.contains("--endpoint"));
    Ok(())
}

#[test]
fn echo_rejects_missing_payload() -> TestResult {
    // Given
    let state = TempState::new()?;
    let mut command = ma2a();

    // When
    let output = command
        .args([
            "--state-dir",
            state.0.to_str().ok_or("state path is not UTF-8")?,
            "echo",
            "--endpoint",
            "0000000000000000000000000000000000000000000000000000000000000000",
        ])
        .output()?;

    // Then
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr)?;
    assert!(stderr.contains("--text <TEXT>") && stderr.contains("--stdin"));
    Ok(())
}

#[test]
fn sync_requires_an_endpoint_target() -> TestResult {
    // Given
    let mut command = ma2a();

    // When
    let output = command.args(["space", "sync", "status"]).output()?;

    // Then
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8(output.stderr)?.contains("--endpoint"));
    Ok(())
}

#[test]
fn private_relay_requires_an_explicit_tls_mode() -> TestResult {
    // Given
    let mut command = ma2a();

    // When
    let output = command
        .args([
            "relay",
            "private",
            "configure",
            "--listen",
            "127.0.0.1:0",
            "--public-url",
            "https://relay.example",
            "--serve-space",
            "0000000000000000000000000000000000000000000000000000000000000000",
        ])
        .output()?;

    // Then
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr)?;
    assert!(stderr.contains("--tls-cert") && stderr.contains("--external-tls"));
    Ok(())
}
