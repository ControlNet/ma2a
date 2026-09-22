use super::{Fixture, TestResult};

#[cfg(unix)]
use std::{
    fs::{self, OpenOptions},
    io::{Read as _, Write as _},
    os::unix::{fs::PermissionsExt as _, net::UnixListener},
    thread::JoinHandle,
};

#[cfg(unix)]
fn incompatible_daemon(
    fixture: &Fixture,
) -> Result<JoinHandle<std::io::Result<()>>, Box<dyn std::error::Error + Send + Sync>> {
    fs::create_dir(fixture.runtime_dir())?;
    fs::set_permissions(fixture.runtime_dir(), fs::Permissions::from_mode(0o700))?;
    let daemon_lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(fixture.runtime_dir().join("daemon.lock"))?;
    daemon_lock.lock()?;
    let endpoint = fixture.runtime_dir().join("control.sock");
    let listener = UnixListener::bind(&endpoint)?;
    fs::set_permissions(&endpoint, fs::Permissions::from_mode(0o600))?;
    Ok(std::thread::spawn(move || {
        let _daemon_lock = daemon_lock;
        loop {
            let (mut stream, _address) = listener.accept()?;
            let mut header = [0_u8; 12];
            stream.read_exact(&mut header)?;
            let length = u32::from_be_bytes([header[0], header[1], header[2], header[3]]);
            let mut payload = vec![0_u8; usize::try_from(length).unwrap_or(0)];
            stream.read_exact(&mut payload)?;
            let request: serde_json::Value = serde_json::from_slice(&payload)?;
            let operation = request.get("operation").and_then(serde_json::Value::as_str);
            let response = if operation == Some("graceful_shutdown") {
                serde_json::json!({
                    "version": 2,
                    "request_id": request.get("request_id"),
                    "revision": 0,
                    "result": {"type": "shutting_down"},
                })
            } else {
                serde_json::json!({
                    "version": 2,
                    "result": {"type": "handshake"},
                })
            };
            let response = serde_json::to_vec(&response)?;
            stream.write_all(&u32::try_from(response.len()).unwrap_or(0).to_be_bytes())?;
            stream.write_all(&header[4..])?;
            stream.write_all(&response)?;
            if operation == Some("graceful_shutdown") {
                return Ok(());
            }
        }
    }))
}

#[test]
fn operational_commands_fail_without_creating_daemon_state() -> TestResult {
    let fixture = Fixture::new()?;
    let commands: &[&[&str]] = &[
        &["status", "--json"],
        &["endpoint", "show"],
        &["space", "list"],
        &["space", "create", "Explicit lifecycle"],
        &["space", "show", "Explicit lifecycle"],
        &["space", "invite", "Explicit lifecycle"],
        &["space", "accept"],
        &["space", "leave", "Explicit lifecycle"],
        &["relay", "private", "status"],
        &["relay", "public", "status"],
        &[
            "echo",
            "--endpoint",
            "0000000000000000000000000000000000000000000000000000000000000000",
            "--text",
            "hello",
        ],
        &["ui", "init"],
        &["ui", "revoke-all"],
        &["ui", "start"],
        &["ui", "status"],
        &["ui", "stop"],
        &["stop"],
    ];
    for arguments in commands {
        let (operation, remaining) = arguments
            .split_first()
            .ok_or("empty lifecycle command fixture")?;
        let output = fixture.command(operation).args(remaining).output()?;
        assert_eq!(output.status.code(), Some(1), "{arguments:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("daemon is not running"),
            "{arguments:?}"
        );
        assert!(output.stdout.is_empty(), "{arguments:?}");
        assert_eq!(
            std::fs::read_dir(&fixture.state_dir)?.count(),
            0,
            "{arguments:?} created state"
        );
    }
    Ok(())
}

#[test]
fn start_is_idempotent_and_restart_preserves_identity_with_a_new_boot() -> TestResult {
    let fixture = Fixture::new()?;
    assert!(fixture.run("restart")?.status.success());
    let first = fixture.run_status()?;
    let first_boot = first
        .get("runtime_boot_id")
        .and_then(serde_json::Value::as_str)
        .ok_or("missing initial Runtime boot ID")?;
    assert!(fixture.run("start")?.status.success());
    let repeated = fixture.run_status()?;
    let repeated_boot = repeated
        .get("runtime_boot_id")
        .and_then(serde_json::Value::as_str)
        .ok_or("missing repeated Runtime boot ID")?;
    assert_eq!(first_boot, repeated_boot);

    assert!(fixture.run("restart")?.status.success());
    let restarted = fixture.run_status()?;
    let restarted_boot = restarted
        .get("runtime_boot_id")
        .and_then(serde_json::Value::as_str)
        .ok_or("missing restarted Runtime boot ID")?;
    assert_ne!(first_boot, restarted_boot);
    assert_eq!(
        first.pointer("/result/payload/endpoint/endpoint_id"),
        restarted.pointer("/result/payload/endpoint/endpoint_id")
    );

    assert!(fixture.run("stop")?.status.success());
    let status = fixture.run("status")?;
    assert!(!status.status.success());
    assert!(String::from_utf8_lossy(&status.stderr).contains("daemon is not running"));
    Ok(())
}

#[test]
fn old_shutdown_command_is_removed() -> TestResult {
    let fixture = Fixture::new()?;
    assert_eq!(fixture.run("shutdown")?.status.code(), Some(2));
    assert!(!fixture.runtime_dir().exists());
    Ok(())
}

#[cfg(unix)]
#[test]
fn lifecycle_commands_replace_or_stop_an_incompatible_daemon() -> TestResult {
    for operation in ["start", "restart", "stop"] {
        let fixture = Fixture::new()?;
        let server = incompatible_daemon(&fixture)?;

        let output = fixture.run(operation)?;

        assert!(
            output.status.success(),
            "{operation}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("daemon protocol is incompatible"),
            "{operation}"
        );
        server
            .join()
            .map_err(|_| "incompatible daemon server panicked")??;
        if operation == "stop" {
            let status = fixture.run("status")?;
            assert!(!status.status.success());
        } else {
            let status = fixture.run_status()?;
            assert_eq!(
                status
                    .pointer("/result/type")
                    .and_then(serde_json::Value::as_str),
                Some("snapshot")
            );
        }
    }
    Ok(())
}
