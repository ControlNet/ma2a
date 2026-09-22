//! Endpoint-centric command-surface regression coverage.

use std::{
    io::Write,
    process::{Command, Stdio},
};

#[path = "support/daemon_fixture.rs"]
mod daemon_fixture;

// These tests are about the command surface alone and never start a daemon, but
// they still take their state directory from the shared fixture so that the rule
// about not removing a directory a daemon owns lives in exactly one place.
use daemon_fixture::{DaemonFixture, TestResult};

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
    for workflow in [
        "start", "restart", "stop", "endpoint", "space", "relay", "echo", "ui",
    ] {
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
fn ui_help_lists_explicit_lifecycle() -> TestResult {
    // Given
    let mut command = ma2a();

    // When
    let output = command.args(["ui", "--help"]).output()?;

    // Then
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout)?;
    assert!(
        stdout.contains("start") && stdout.contains("stop") && stdout.contains("status"),
        "missing lifecycle commands in UI help:\n{stdout}"
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
    let state = DaemonFixture::idle("cli-echo")?;
    let mut command = ma2a();

    // When
    let output = command
        .args([
            "--state-dir",
            state
                .state_dir()
                .to_str()
                .ok_or("state path is not UTF-8")?,
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
    state.shutdown()
}

#[test]
fn echo_accepts_endpoint_stdin_json() -> TestResult {
    // Given
    let state = DaemonFixture::idle("cli-echo")?;
    let mut command = ma2a();
    let mut child = command
        .args([
            "--state-dir",
            state
                .state_dir()
                .to_str()
                .ok_or("state path is not UTF-8")?,
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
    state.shutdown()
}

#[test]
fn echo_accepts_endpoint_stdin() -> TestResult {
    // Given
    let state = DaemonFixture::idle("cli-echo")?;
    let mut command = ma2a();
    let mut child = command
        .args([
            "--state-dir",
            state
                .state_dir()
                .to_str()
                .ok_or("state path is not UTF-8")?,
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
    state.shutdown()
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
    let state = DaemonFixture::idle("cli-echo")?;
    let mut command = ma2a();

    // When
    let output = command
        .args([
            "--state-dir",
            state
                .state_dir()
                .to_str()
                .ok_or("state path is not UTF-8")?,
            "echo",
            "--endpoint",
            "0000000000000000000000000000000000000000000000000000000000000000",
        ])
        .output()?;

    // Then
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr)?;
    assert!(stderr.contains("--text <TEXT>") && stderr.contains("--stdin"));
    state.shutdown()
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
