//! Daemons of another version, and daemons from before the lifecycle plane.
//!
//! Upgrading the executable must never leave a daemon nothing can stop, and no
//! lifecycle command may wait for good on a daemon that will not answer. These
//! cover both halves: which plane each kind of daemon is stopped through, and
//! that the one plane with no deadline of its own is given one.

use std::{
    process::{Child, Output},
    time::{Duration, Instant},
};

use serde_json::Value;

use super::{
    DaemonFixture, TestResult, TestValue,
    fakes::{Fake, FakeDaemon, Plane, Shutdown},
    is_owned, status_json,
};

/// Longest any lifecycle command may take against a daemon that never replies.
///
/// It covers the lifecycle probe, the versioned handshake and the ten-second
/// compatibility deadline, with room for a loaded runner. A command still
/// running past this is not bounded by anything this executable controls.
const BOUNDED: Duration = Duration::from_secs(40);

/// A newer daemon speaks another business API but the same lifecycle plane.
#[test]
fn an_incompatible_daemon_that_speaks_the_lifecycle_plane_is_stopped_through_it() -> TestResult {
    for operation in ["stop", "restart", "start"] {
        // Given
        let mut fixture = DaemonFixture::idle("lifecycle-incompatible-capable")?;
        let fake = FakeDaemon::serve(
            &fixture,
            Fake {
                plane: Plane::Lifecycle,
                api_version: 2,
                shutdown: Shutdown::Ignore,
                owns_lock: true,
            },
        )?;

        // When
        let output = fixture.run(&[operation])?;
        if operation != "stop" {
            fixture.adopt_detached();
        }

        // Then
        assert!(
            output.status.success(),
            "{operation}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(fake.has_exited(), "{operation} must have stopped it");
        let asked = fake.finish()?;
        assert!(
            asked.iter().any(|request| request == "lifecycle:stop"),
            "{operation} must use the lifecycle plane: {asked:?}"
        );
        assert!(
            !asked
                .iter()
                .any(|request| request == "graceful_shutdown" || request == "handshake"),
            "{operation} must not fall back to the business plane: {asked:?}"
        );
        if operation == "stop" {
            assert!(!is_owned(&fixture)?);
        } else {
            assert_eq!(
                status_json(&fixture)?
                    .pointer("/result/type")
                    .and_then(Value::as_str),
                Some("snapshot"),
                "{operation} must leave the replacement serving"
            );
        }
        fixture.shutdown()?;
    }
    Ok(())
}

/// An old daemon whose Runtime never answers cannot make any lifecycle command hang.
#[test]
fn a_legacy_daemon_that_never_acknowledges_shutdown_cannot_hang_a_lifecycle_command() -> TestResult
{
    // Given: every combination at once, so the bound is paid once, not per case.
    let cases = [
        (1, "stop"),
        (1, "restart"),
        (2, "stop"),
        (2, "restart"),
        (2, "start"),
    ];
    let mut running = Vec::new();
    for (api_version, operation) in cases {
        let fixture = DaemonFixture::idle("lifecycle-legacy-wedged")?;
        let fake = FakeDaemon::serve(
            &fixture,
            Fake {
                plane: Plane::Legacy,
                api_version,
                shutdown: Shutdown::Ignore,
                owns_lock: true,
            },
        )?;
        let child = fixture.command(&[operation]).spawn()?;
        running.push((api_version, operation, fixture, fake, child));
    }

    // When
    let started = Instant::now();
    let finished = running
        .into_iter()
        .map(|(api_version, operation, fixture, fake, child)| {
            finish_within(child, started)
                .map(|output| (api_version, operation, fixture, fake, output))
        })
        .collect::<TestValue<Vec<_>>>()?;

    // Then
    for (api_version, operation, fixture, fake, (output, elapsed)) in finished {
        let case = format!("API version {api_version}, {operation}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(elapsed < BOUNDED, "{case} took {elapsed:?}");
        assert!(!output.status.success(), "{case} must fail: {stderr}");
        assert!(
            stderr.contains("did not answer the compatible stop request"),
            "{case} must say why: {stderr}"
        );
        assert!(!fake.has_exited(), "{case}: nothing may have replaced it");
        let asked = fake.finish()?;
        assert!(
            asked.iter().any(|request| request == "graceful_shutdown"),
            "{case} must have asked it through the command it understands: {asked:?}"
        );
        fixture.shutdown()?;
    }
    Ok(())
}

/// An old daemon that does acknowledge is still stopped, and proven gone.
#[test]
fn a_legacy_daemon_of_this_api_version_is_stopped_through_graceful_shutdown() -> TestResult {
    // Given
    let fixture = DaemonFixture::idle("lifecycle-legacy-current")?;
    let fake = FakeDaemon::serve(
        &fixture,
        Fake {
            plane: Plane::Legacy,
            api_version: 1,
            shutdown: Shutdown::Acknowledge,
            owns_lock: true,
        },
    )?;

    // When
    let output = fixture.run(&["stop"])?;

    // Then
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!is_owned(&fixture)?);
    assert!(!fixture.runtime_dir().join("control.sock").exists());
    let asked = fake.finish()?;
    assert!(asked.iter().any(|request| request == "graceful_shutdown"));
    fixture.shutdown()
}

/// A daemon that owns the directory never unlinks an endpoint it refused to replace.
#[test]
fn a_daemon_that_refuses_a_live_endpoint_leaves_it_in_place() -> TestResult {
    // Given: something answering the endpoint without owning the directory.
    let fixture = DaemonFixture::idle("lifecycle-refused-endpoint")?;
    let fake = FakeDaemon::serve(
        &fixture,
        Fake {
            plane: Plane::Legacy,
            api_version: 1,
            shutdown: Shutdown::Ignore,
            owns_lock: false,
        },
    )?;

    // When
    let output = fixture.run(&["daemon"])?;

    // Then
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("refusing to replace it"), "{stderr}");
    assert!(
        fixture.runtime_dir().join("control.sock").exists(),
        "the endpoint it refused must still be there after it exits"
    );
    assert!(!is_owned(&fixture)?, "the refusing daemon must own nothing");
    assert!(
        !fixture.runtime_dir().join("daemon.json").exists(),
        "the refusing daemon must not leave its record behind"
    );
    let asked = fake.finish()?;
    assert!(asked.iter().any(|request| request == "handshake"));
    fixture.shutdown()
}

/// Waits for a command, killing it if it outlives any legitimate bound.
fn finish_within(mut child: Child, started: Instant) -> TestValue<(Output, Duration)> {
    let give_up = started + BOUNDED * 2;
    while child.try_wait()?.is_none() {
        if Instant::now() >= give_up {
            let _killed = child.kill();
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let output = child.wait_with_output()?;
    Ok((output, started.elapsed()))
}
