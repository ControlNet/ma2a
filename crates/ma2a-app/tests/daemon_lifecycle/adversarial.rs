//! The lifecycle cases that used to leave daemons behind.

use std::{
    fs::{self, OpenOptions},
    process::Stdio,
    time::{Duration, Instant},
};

use serde_json::Value;

use super::{DaemonFixture, TestResult, TestValue, daemon_record, is_owned};

/// Holds the daemon lock the way a live daemon does, without serving anything.
///
/// This is the state a caller cannot tell apart from a healthy daemon by asking
/// the endpoint, and the one an earlier revision rounded down to "no daemon".
fn owned_but_silent(fixture: &DaemonFixture) -> TestValue<fs::File> {
    let runtime_dir = fixture.runtime_dir();
    fs::create_dir_all(&runtime_dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&runtime_dir, fs::Permissions::from_mode(0o700))?;
    }
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(runtime_dir.join("daemon.lock"))?;
    lock.lock()?;
    Ok(lock)
}

#[test]
fn an_unresponsive_owner_is_never_mistaken_for_an_absent_one() -> TestResult {
    // Given
    let fixture = DaemonFixture::idle("lifecycle-unresponsive")?;
    let held = owned_but_silent(&fixture)?;

    // When
    let outcomes = ["start", "restart", "stop", "status"]
        .map(|operation| fixture.run(&[operation]))
        .into_iter()
        .collect::<TestValue<Vec<_>>>()?;

    // Then
    for (operation, output) in ["start", "restart", "stop", "status"].iter().zip(&outcomes) {
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "{operation} must fail closed");
        assert!(
            stderr.contains("not answering"),
            "{operation} must name the unresponsive owner: {stderr}"
        );
        assert!(
            !stderr.contains("daemon is not running"),
            "{operation} must not report absence: {stderr}"
        );
    }
    assert!(
        !fixture.runtime_dir().join("control.sock").exists(),
        "nothing may have been started beside the existing owner"
    );

    // And once the owner goes, the directory is plainly absent again.
    drop(held);
    let absent = fixture.run(&["status"])?;
    assert!(
        String::from_utf8_lossy(&absent.stderr).contains("daemon is not running"),
        "{}",
        String::from_utf8_lossy(&absent.stderr)
    );
    fixture.shutdown()
}

#[test]
fn a_held_lock_protects_an_endpoint_that_no_probe_can_reach() -> TestResult {
    // Given: an owner that answers nothing, and an endpoint path beside it.
    let fixture = DaemonFixture::idle("lifecycle-protected-endpoint")?;
    let held = owned_but_silent(&fixture)?;
    let endpoint = fixture.runtime_dir().join("control.sock");
    fs::write(&endpoint, b"owned by the silent daemon")?;

    // When
    let started = fixture.run(&["start"])?;

    // Then: a failed probe is never a licence to unlink a live owner's endpoint.
    assert!(!started.status.success());
    assert_eq!(fs::read(&endpoint)?, b"owned by the silent daemon");
    drop(held);
    fs::remove_file(&endpoint)?;
    fixture.shutdown()
}

#[test]
fn a_free_lock_beside_a_stale_endpoint_is_reclaimed_by_one_daemon() -> TestResult {
    // Given
    let mut fixture = DaemonFixture::idle("lifecycle-stale-endpoint")?;
    let runtime_dir = fixture.runtime_dir();
    fs::create_dir(&runtime_dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&runtime_dir, fs::Permissions::from_mode(0o700))?;
    }
    fs::write(runtime_dir.join("control.sock"), b"stale")?;

    // When
    assert!(fixture.start_detached()?.status.success());

    // Then
    assert_eq!(
        fixture
            .json(&["status", "--json"])?
            .pointer("/result/type")
            .and_then(Value::as_str),
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
    fixture.shutdown()
}

/// A daemon whose launcher has gone must not carry on as an orphan.
#[cfg(unix)]
#[test]
fn a_daemon_that_cannot_report_readiness_stops_instead_of_detaching() -> TestResult {
    // Given
    let fixture = DaemonFixture::idle("lifecycle-lost-launcher")?;
    let mut child = fixture
        .command(&["daemon-detached"])
        .stdout(Stdio::piped())
        .spawn()?;

    // When: the launcher disappears before readiness can be delivered.
    drop(child.stdout.take());

    // Then
    let status = child.wait()?;
    assert!(
        !status.success(),
        "a daemon that could not report readiness must fail"
    );
    assert!(!is_owned(&fixture)?, "it must not still own the directory");
    assert!(
        !fixture.runtime_dir().join("control.sock").exists(),
        "it must not leave an endpoint behind"
    );
    assert!(!fixture.runtime_dir().join("daemon.json").exists());
    fixture.shutdown()
}

/// A start that fails must leave nothing running, and must say why.
#[cfg(unix)]
#[test]
fn a_failed_start_reaps_its_child_and_releases_the_directory() -> TestResult {
    // Given: the daemon refuses a state directory anyone else can read. Reaching
    // that refusal is the point: it happens after the parent has detached, so
    // the reason exists only on the child's standard error.
    use std::os::unix::fs::PermissionsExt as _;

    let fixture = DaemonFixture::idle("lifecycle-failed-start")?;
    fs::set_permissions(fixture.state_dir(), fs::Permissions::from_mode(0o755))?;

    // When
    let started = fixture.run(&["start"])?;

    // Then
    assert!(!started.status.success());
    let recorded = fs::read_to_string(fixture.runtime_dir().join("daemon.log"))?;
    assert!(recorded.contains("state directory"), "{recorded}");
    assert!(
        String::from_utf8_lossy(&started.stderr).contains("state directory"),
        "the failure must carry the daemon's own reason: {}",
        String::from_utf8_lossy(&started.stderr)
    );
    assert!(!fixture.runtime_dir().join("control.sock").exists());
    assert!(!is_owned(&fixture)?, "a failed start must own nothing");

    // No daemon may appear afterwards either.
    let settle = Instant::now() + Duration::from_millis(500);
    while Instant::now() < settle {
        assert!(!is_owned(&fixture)?);
        std::thread::sleep(Duration::from_millis(50));
    }
    fs::set_permissions(fixture.state_dir(), fs::Permissions::from_mode(0o700))?;
    fixture.shutdown()
}

/// The fixture's own promise: nothing is removed until the daemon is proven gone.
#[cfg(target_os = "linux")]
#[test]
fn the_fixture_reaps_its_daemon_before_removing_the_state_directory() -> TestResult {
    // Given
    let fixture = DaemonFixture::running("lifecycle-owned-teardown")?;
    let state_dir = fixture.state_dir().to_path_buf();
    let pid = daemon_record(&fixture)?
        .get("pid")
        .and_then(Value::as_u64)
        .ok_or("the daemon record carries no process identifier")?;
    assert!(super::daemon_process_is_live(pid));
    // A failing command must not change any of this.
    assert!(!fixture.run(&["space", "show", "absent"])?.status.success());

    // When
    fixture.shutdown()?;

    // Then
    assert!(
        !super::daemon_process_is_live(pid),
        "process {pid} outlived the fixture that owned it"
    );
    assert!(!state_dir.exists(), "the state directory was left behind");
    Ok(())
}

/// Something answering an endpoint nobody owns is ambiguous, not an invitation.
#[cfg(unix)]
#[test]
fn an_unowned_endpoint_that_still_answers_is_reported_as_ambiguous() -> TestResult {
    use std::{
        io::{Read as _, Write as _},
        os::unix::{fs::PermissionsExt as _, net::UnixListener},
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
    };

    // Given: a responder that never took the daemon lock.
    let fixture = DaemonFixture::idle("lifecycle-conflicted")?;
    let runtime_dir = fixture.runtime_dir();
    fs::create_dir(&runtime_dir)?;
    fs::set_permissions(&runtime_dir, fs::Permissions::from_mode(0o700))?;
    let endpoint = runtime_dir.join("control.sock");
    let listener = UnixListener::bind(&endpoint)?;
    fs::set_permissions(&endpoint, fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    let stopped = Arc::new(AtomicBool::new(false));
    let serving = Arc::clone(&stopped);
    let responder = std::thread::spawn(move || -> std::io::Result<()> {
        while !serving.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((mut stream, _address)) => {
                    let mut header = [0_u8; 12];
                    stream.read_exact(&mut header)?;
                    let length = u32::from_be_bytes([header[0], header[1], header[2], header[3]]);
                    let mut payload = vec![0_u8; usize::try_from(length).unwrap_or(0)];
                    stream.read_exact(&mut payload)?;
                    let reply = br#"{"version":1,"request_id":null,"revision":0,"result":{"type":"handshake"}}"#;
                    stream.write_all(&u32::try_from(reply.len()).unwrap_or(0).to_be_bytes())?;
                    stream.write_all(&header[4..])?;
                    stream.write_all(reply)?;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::yield_now();
                }
                Err(error) => return Err(error),
            }
        }
        Ok(())
    });

    // When
    let started = fixture.run(&["start"])?;

    // Then
    stopped.store(true, Ordering::Relaxed);
    responder
        .join()
        .map_err(|_| "the unowned responder panicked")??;
    assert!(!started.status.success());
    let stderr = String::from_utf8_lossy(&started.stderr);
    assert!(stderr.contains("ambiguous"), "{stderr}");
    assert!(
        endpoint.exists(),
        "an endpoint something is using must not be unlinked"
    );
    fs::remove_file(&endpoint)?;
    fixture.shutdown()
}

/// The launch marker belongs to the launch, not to whatever was in the environment.
#[test]
fn a_launch_generates_its_own_marker_regardless_of_the_environment() -> TestResult {
    // Given
    let mut fixture = DaemonFixture::idle("lifecycle-launch-marker")?;
    let planted = "ffffffffffffffffffffffffffffffff";

    // When
    let started = fixture
        .command(&["start"])
        .env("MA2A_LAUNCH_NONCE", planted)
        .output()?;
    fixture.adopt_detached();

    // Then
    assert!(
        started.status.success(),
        "{}",
        String::from_utf8_lossy(&started.stderr)
    );
    let recorded = daemon_record(&fixture)?;
    let nonce = recorded
        .get("launch_nonce")
        .and_then(Value::as_str)
        .ok_or("the daemon record carries no launch marker")?;
    assert_ne!(
        nonce, planted,
        "the launcher must mint its own marker, not adopt an inherited one"
    );
    assert_eq!(nonce.len(), 32);
    fixture.shutdown()
}
