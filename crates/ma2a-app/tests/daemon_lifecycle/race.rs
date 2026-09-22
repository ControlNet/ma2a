//! `ma2a start` racing a foreground `ma2a daemon` for the same directory.
//!
//! The foreground daemon does not take the startup lock, so a launcher can
//! decide that nothing owns the directory and then find that something does.
//! Whatever the interleaving, the directory must end with exactly one owner and
//! that owner must still be addressable.

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

    /// Waits until the launcher is between classifying the directory and launching.
    fn await_reached(&self) -> TestResult {
        let deadline = Instant::now() + Duration::from_secs(60);
        while !self.0.join("reached").exists() {
            if Instant::now() >= deadline {
                return Err("the launcher never reached its launch".into());
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

/// The adversarial order: the foreground daemon wins inside the launcher's window.
///
/// The launcher has already seen a free lock and no endpoint. Removing the
/// endpoint on that basis, as an earlier revision did before launching, unlinks
/// the socket of the daemon that won in the meantime and leaves it owning the
/// directory with no address anyone can reach.
#[cfg(debug_assertions)]
#[test]
fn a_launch_that_loses_to_a_foreground_daemon_leaves_the_winner_addressable() -> TestResult {
    // Given: a launcher paused after deciding the directory is unowned.
    let mut fixture = DaemonFixture::idle("lifecycle-start-vs-daemon")?;
    let hold = Hold::new(&fixture)?;
    let launcher = fixture
        .command(&["start"])
        .env(HOLD_VARIABLE, hold.path())
        .spawn()?;
    hold.await_reached()?;

    // When: a foreground daemon claims the directory, and only then the launcher goes on.
    fixture.start_owned()?;
    let winner = fixture
        .owned_pid()
        .ok_or("the foreground daemon has no process")?;
    hold.release()?;
    let outcome = launcher.wait_with_output()?;

    // Then: the launch failed, and failed because the directory was already owned.
    let stderr = String::from_utf8_lossy(&outcome.stderr);
    assert!(!outcome.status.success(), "the losing launch must fail");
    assert!(stderr.contains("another daemon already owns"), "{stderr}");

    // And the winner is untouched: still running, still owning, still reachable.
    assert!(fixture.owned_daemon_is_running());
    assert!(is_owned(&fixture)?);
    assert_is_socket(&fixture.runtime_dir().join("control.sock"))?;
    assert_eq!(
        status_json(&fixture)?
            .pointer("/result/type")
            .and_then(Value::as_str),
        Some("snapshot")
    );
    let record = daemon_record(&fixture)?;
    assert_eq!(
        record.get("pid").and_then(Value::as_u64),
        Some(u64::from(winner))
    );
    assert_eq!(record.get("launch_nonce"), Some(&Value::Null));
    assert!(
        fixture.run_ok(&["stop"]).is_ok(),
        "the winner must stay stoppable"
    );
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
