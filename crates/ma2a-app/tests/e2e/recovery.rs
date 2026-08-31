use std::{
    fs,
    process::{Command, Stdio},
};

use super::harness::{TempState, TestResult};

#[test]
fn stale_ipc_endpoint_is_recovered_by_a_real_daemon_process() -> TestResult {
    // Given
    let state = TempState::new("recovery")?;
    let state_path = state
        .config()
        .database_path()
        .parent()
        .ok_or("state path missing")?
        .to_owned();
    let runtime_dir = state_path.join("run-v1");
    fs::create_dir(&runtime_dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&runtime_dir, fs::Permissions::from_mode(0o700))?;
    }
    fs::write(runtime_dir.join("control.sock"), b"stale")?;

    // When
    let output = Command::new(env!("CARGO_BIN_EXE_ma2a"))
        .arg("--state-dir")
        .arg(&state_path)
        .arg("status")
        .arg("--json")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()?;

    // Then
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(
        response
            .pointer("/result/type")
            .and_then(serde_json::Value::as_str),
        Some("snapshot")
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt as _;
        assert!(
            fs::metadata(runtime_dir.join("control.sock"))?
                .file_type()
                .is_socket()
        );
    }
    let shutdown = Command::new(env!("CARGO_BIN_EXE_ma2a"))
        .arg("--state-dir")
        .arg(&state_path)
        .arg("shutdown")
        .output()?;
    assert!(shutdown.status.success());
    Ok(())
}
