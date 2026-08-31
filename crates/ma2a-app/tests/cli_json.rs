//! Stable machine-readable CLI output coverage.

use std::{
    error::Error,
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

use serde_json::Value;

type TestResult = Result<(), Box<dyn Error>>;

static NEXT_STATE: AtomicU64 = AtomicU64::new(0);

#[test]
fn status_json_uses_the_local_api_envelope() -> TestResult {
    // Given
    let serial = NEXT_STATE.fetch_add(1, Ordering::Relaxed);
    let state_dir = state_dir(serial)?;

    // When
    let output = Command::new(env!("CARGO_BIN_EXE_ma2a"))
        .arg("--state-dir")
        .arg(&state_dir)
        .args(["status", "--json"])
        .output()?;

    // Then
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(
        response.pointer("/result/type").and_then(Value::as_str),
        Some("snapshot")
    );

    let _shutdown = Command::new(env!("CARGO_BIN_EXE_ma2a"))
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("shutdown")
        .output();
    fs::remove_dir_all(state_dir)?;
    Ok(())
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "the process scenario keeps creation, restart, and durable label proofs visible"
)]
fn space_create_commits_membership_through_the_runtime() -> TestResult {
    // Given
    let serial = NEXT_STATE.fetch_add(1, Ordering::Relaxed);
    let state_dir = state_dir(serial)?;

    // When
    let created = Command::new(env!("CARGO_BIN_EXE_ma2a"))
        .arg("--state-dir")
        .arg(&state_dir)
        .args(["space", "create", "--name", "Personal", "--json"])
        .output()?;

    // Then
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stderr)
    );
    let response: Value = serde_json::from_slice(&created.stdout)?;
    assert_eq!(
        response.pointer("/result/type").and_then(Value::as_str),
        Some("space_created")
    );
    assert_eq!(
        response
            .pointer("/result/payload/name")
            .and_then(Value::as_str),
        Some("Personal")
    );
    let created_revision = response
        .get("revision")
        .and_then(Value::as_u64)
        .ok_or("missing creation revision")?;
    let space_id = response
        .pointer("/result/payload/space_id")
        .and_then(Value::as_str)
        .ok_or("missing created Space ID")?;
    let listed = Command::new(env!("CARGO_BIN_EXE_ma2a"))
        .arg("--state-dir")
        .arg(&state_dir)
        .args(["space", "list", "--json"])
        .output()?;
    let list_response: Value = serde_json::from_slice(&listed.stdout)?;
    assert_eq!(
        list_response
            .pointer("/result/type")
            .and_then(Value::as_str),
        Some("spaces")
    );
    let shown = Command::new(env!("CARGO_BIN_EXE_ma2a"))
        .arg("--state-dir")
        .arg(&state_dir)
        .args(["space", "show", "--space", space_id, "--json"])
        .output()?;
    let show_response: Value = serde_json::from_slice(&shown.stdout)?;
    assert_eq!(
        show_response
            .pointer("/result/type")
            .and_then(Value::as_str),
        Some("space")
    );
    let status = Command::new(env!("CARGO_BIN_EXE_ma2a"))
        .arg("--state-dir")
        .arg(&state_dir)
        .args(["status", "--json"])
        .output()?;
    let snapshot: Value = serde_json::from_slice(&status.stdout)?;
    assert_eq!(
        snapshot.get("revision").and_then(Value::as_u64),
        Some(created_revision)
    );
    assert_eq!(
        snapshot
            .pointer("/result/payload/spaces")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(1)
    );
    assert_eq!(
        snapshot
            .pointer("/result/payload/spaces/0/name")
            .and_then(Value::as_str),
        Some("Personal")
    );

    let _shutdown = Command::new(env!("CARGO_BIN_EXE_ma2a"))
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("shutdown")
        .output();
    let restarted = Command::new(env!("CARGO_BIN_EXE_ma2a"))
        .arg("--state-dir")
        .arg(&state_dir)
        .args(["space", "list", "--json"])
        .output()?;
    let restarted_response: Value = serde_json::from_slice(&restarted.stdout)?;
    assert_eq!(
        restarted_response
            .pointer("/result/payload/0/name")
            .and_then(Value::as_str),
        Some("Personal")
    );

    let _shutdown = Command::new(env!("CARGO_BIN_EXE_ma2a"))
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("shutdown")
        .output();
    fs::remove_dir_all(state_dir)?;
    Ok(())
}

fn state_dir(serial: u64) -> Result<PathBuf, std::io::Error> {
    let path = std::env::temp_dir().join(format!("ma2a-cli-json-{}-{serial}", std::process::id()));
    fs::create_dir(&path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(path)
}
