//! Endpoint-centric command-surface regression coverage.

use std::{error::Error, process::Command};

type TestResult = Result<(), Box<dyn Error>>;

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
