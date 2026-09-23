//! Daemon-generation transitions racing foreground startup.

use std::{fs, path::Path};
#[cfg(debug_assertions)]
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use serde_json::Value;

#[cfg(debug_assertions)]
use super::TestValue;
use super::{DaemonFixture, TestResult, daemon_record, is_owned, status_json};

#[cfg(debug_assertions)]
/// The debug-build pause point between a launcher's decision and its launch.
const HOLD_VARIABLE: &str = "MA2A_TEST_HOLD_BEFORE_SPAWN";
#[cfg(debug_assertions)]
const STOP_HOLD_VARIABLE: &str = "MA2A_TEST_HOLD_BEFORE_STOP";
#[cfg(debug_assertions)]
const CONTENDED_VARIABLE: &str = "MA2A_TEST_SIGNAL_STARTUP_LOCK_CONTENDED";

#[cfg(debug_assertions)]
/// A private directory the held launcher and the test signal each other through.
struct Hold(PathBuf);

#[cfg(debug_assertions)]
impl Hold {
    fn new(fixture: &DaemonFixture) -> TestValue<Self> {
        let name = fixture
            .state_dir()
            .file_name()
            .ok_or("the fixture has no directory name")?;
        let path = std::env::temp_dir().join(format!("{}-hold", name.to_string_lossy()));
        fs::create_dir(&path)?;
        Ok(Self(path))
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn await_marker(&self, marker: &str) -> TestResult {
        let deadline = Instant::now() + Duration::from_secs(60);
        while !self.0.join(marker).exists() {
            if Instant::now() >= deadline {
                return Err(format!("the process never reported {marker}").into());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Ok(())
    }

    fn release(&self) -> TestResult {
        Ok(fs::write(self.0.join("release"), b"")?)
    }
}

#[cfg(debug_assertions)]
impl Drop for Hold {
    fn drop(&mut self) {
        let _released = fs::write(self.0.join("release"), b"");
        let _removed = fs::remove_dir_all(&self.0);
    }
}

/// A foreground daemon cannot enter after start has classified absence.
#[cfg(debug_assertions)]
#[test]
fn a_held_start_launches_before_a_contending_foreground_daemon() -> TestResult {
    // Given: a launcher paused after deciding the directory is unowned.
    let mut fixture = DaemonFixture::idle("lifecycle-start-vs-daemon")?;
    let hold = Hold::new(&fixture)?;
    let launcher = fixture
        .command(&["start"])
        .env(HOLD_VARIABLE, hold.path())
        .spawn()?;
    hold.await_marker("reached")?;

    // The foreground process reaches the held lock before the launcher proceeds.
    let foreground = fixture
        .command(&["daemon"])
        .env(CONTENDED_VARIABLE, hold.path())
        .spawn()?;
    hold.await_marker("contended")?;
    assert!(!is_owned(&fixture)?);
    hold.release()?;
    let outcome = launcher.wait_with_output()?;
    fixture.adopt_detached();
    let foreground_outcome = foreground.wait_with_output()?;

    assert!(
        outcome.status.success(),
        "{}",
        String::from_utf8_lossy(&outcome.stderr)
    );
    let stderr = String::from_utf8_lossy(&foreground_outcome.stderr);
    assert!(!foreground_outcome.status.success());
    assert!(stderr.contains("another daemon already owns"), "{stderr}");

    assert!(is_owned(&fixture)?);
    assert_is_socket(&fixture.runtime_dir().join("control.sock"))?;
    assert_eq!(
        status_json(&fixture)?
            .pointer("/result/type")
            .and_then(Value::as_str),
        Some("snapshot")
    );
    let record = daemon_record(&fixture)?;
    assert!(record.get("launch_nonce").and_then(Value::as_str).is_some());
    assert!(
        fixture.run_ok(&["stop"]).is_ok(),
        "the winning launch must stay stoppable"
    );
    assert!(!is_owned(&fixture)?);
    fixture.shutdown()
}

/// Stop cannot reach a newly arriving daemon after its classified owner exits.
#[cfg(debug_assertions)]
#[test]
fn stop_cannot_cross_from_a_departed_owner_to_a_new_foreground_daemon() -> TestResult {
    let mut fixture = DaemonFixture::running("lifecycle-stop-generation-race")?;
    let hold = Hold::new(&fixture)?;
    let stopper = fixture
        .command(&["stop"])
        .env(STOP_HOLD_VARIABLE, hold.path())
        .spawn()?;
    hold.await_marker("reached")?;

    // A exits independently after stop classified it. B has attempted the
    // transition lock, yet cannot claim the now-free singleton lock.
    fixture.stop_owned()?;
    let foreground = fixture
        .command(&["daemon"])
        .env(CONTENDED_VARIABLE, hold.path())
        .spawn()?;
    hold.await_marker("contended")?;
    assert!(!is_owned(&fixture)?);
    hold.release()?;

    let stop_outcome = stopper.wait_with_output()?;
    assert!(
        !stop_outcome.status.success(),
        "A left before its stop request"
    );
    fixture.adopt_owned_child(foreground)?;
    assert!(is_owned(&fixture)?);
    assert_eq!(
        status_json(&fixture)?
            .pointer("/result/type")
            .and_then(Value::as_str),
        Some("snapshot")
    );
    fixture.shutdown()
}

/// Restart keeps the transition lock through proven release and replacement.
#[cfg(debug_assertions)]
#[test]
fn restart_blocks_foreground_entry_between_old_and_new_generations() -> TestResult {
    let mut fixture = DaemonFixture::idle("lifecycle-restart-generation-race")?;
    assert!(fixture.start_detached()?.status.success());
    let old_pid = daemon_record(&fixture)?.get("pid").and_then(Value::as_u64);
    let hold = Hold::new(&fixture)?;
    let restart = fixture
        .command(&["restart"])
        .env(HOLD_VARIABLE, hold.path())
        .spawn()?;
    hold.await_marker("reached")?;
    assert!(
        !is_owned(&fixture)?,
        "old owner must have released before spawn"
    );

    let foreground = fixture
        .command(&["daemon"])
        .env(CONTENDED_VARIABLE, hold.path())
        .spawn()?;
    hold.await_marker("contended")?;
    assert!(!is_owned(&fixture)?);
    hold.release()?;

    let restart_outcome = restart.wait_with_output()?;
    assert!(
        restart_outcome.status.success(),
        "{}",
        String::from_utf8_lossy(&restart_outcome.stderr)
    );
    let foreground_outcome = foreground.wait_with_output()?;
    assert!(!foreground_outcome.status.success());
    assert!(
        String::from_utf8_lossy(&foreground_outcome.stderr).contains("another daemon already owns")
    );
    let new_record = daemon_record(&fixture)?;
    assert_ne!(new_record.get("pid").and_then(Value::as_u64), old_pid);
    assert!(is_owned(&fixture)?);
    assert_eq!(
        status_json(&fixture)?
            .pointer("/result/type")
            .and_then(Value::as_str),
        Some("snapshot")
    );
    fixture.shutdown()
}

#[test]
fn failed_foreground_startup_releases_the_transition_lock() -> TestResult {
    let fixture = DaemonFixture::running("lifecycle-foreground-failure-lock")?;
    let failed = fixture.run(&["daemon"])?;
    assert!(!failed.status.success());
    assert!(String::from_utf8_lossy(&failed.stderr).contains("another daemon already owns"));
    fixture.run_ok(&["stop"])?;
    assert!(!is_owned(&fixture)?);
    fixture.shutdown()
}

/// Unordered, repeatedly: whoever wins, there is one owner and it answers.
#[test]
fn start_and_a_foreground_daemon_racing_always_leave_one_addressable_owner() -> TestResult {
    for _attempt in 0..5 {
        // Given
        let mut fixture = DaemonFixture::idle("lifecycle-start-daemon-race")?;
        let launcher = fixture.command(&["start"]).spawn()?;

        // When
        let foreground = fixture.start_owned();
        let outcome = launcher.wait_with_output()?;
        let launch_stdout = String::from_utf8_lossy(&outcome.stdout);
        let launch_started = outcome.status.success() && launch_stdout.contains("daemon started");
        if launch_started {
            fixture.adopt_detached();
        }

        // Then: exactly one of them became the owner.
        assert!(
            foreground.is_ok() != launch_started,
            "exactly one owner: foreground {foreground:?}, launch {launch_stdout} {}",
            String::from_utf8_lossy(&outcome.stderr)
        );
        assert!(is_owned(&fixture)?);
        assert_is_socket(&fixture.runtime_dir().join("control.sock"))?;
        let record = daemon_record(&fixture)?;
        let expected = fixture.owned_pid().map(u64::from);
        if let Some(pid) = expected {
            assert_eq!(record.get("pid").and_then(Value::as_u64), Some(pid));
        }
        assert_eq!(
            status_json(&fixture)?
                .pointer("/result/type")
                .and_then(Value::as_str),
            Some("snapshot")
        );
        fixture.shutdown()?;
    }
    Ok(())
}

fn assert_is_socket(path: &Path) -> TestResult {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt as _;
        assert!(
            fs::metadata(path)?.file_type().is_socket(),
            "{} is not a live endpoint",
            path.display()
        );
    }
    #[cfg(not(unix))]
    let _unused = path;
    Ok(())
}
