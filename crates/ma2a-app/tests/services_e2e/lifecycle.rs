//! Resource bounds and restart between real daemons.

use std::{
    process::{Child, Stdio},
    time::{Duration, Instant},
};

use super::{
    daemon_fixture::{TestResult, TestValue},
    harness::{Lab, refused, succeed},
};

/// The Shell sessions one peer may hold on a target at once.
const SHELLS_PER_PEER: usize = 4;

fn eventually(limit: Duration, mut condition: impl FnMut() -> TestValue<bool>) -> TestResult {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if condition()? {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Err("condition did not hold in time".into())
}

fn session(lab: &Lab) -> TestValue<Child> {
    Ok(lab
        .member
        .command(&[
            "shell",
            "--endpoint",
            lab.owner.id(),
            "--",
            "sh",
            "-c",
            "sleep 60",
        ])
        .stdin(Stdio::null())
        .spawn()?)
}

#[test]
fn sessions_are_bounded_and_a_restart_leaves_no_operation_active() -> TestResult {
    let mut lab = Lab::start("bounded", "1")?;
    let mut sessions = (0..SHELLS_PER_PEER)
        .map(|_| session(&lab))
        .collect::<TestValue<Vec<_>>>()?;
    eventually(Duration::from_secs(30), || {
        Ok(lab.owner.active_operations()? == SHELLS_PER_PEER as u64)
    })?;
    let excess = lab
        .member
        .command(&[
            "shell",
            "--endpoint",
            lab.owner.id(),
            "--",
            "sh",
            "-c",
            "true",
        ])
        .stdin(Stdio::null())
        .output()?;
    refused(&excess, "concurrency_exceeded")?;

    // A restart ends every session; none of them is reported as still running.
    lab.owner.restart()?;
    for child in &mut sessions {
        let deadline = Instant::now() + Duration::from_secs(30);
        while child.try_wait()?.is_none() {
            if Instant::now() > deadline {
                return Err("a session outlived the target's restart".into());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let status = child.wait()?;
        assert!(
            !status.success(),
            "a session ended by restart reported success"
        );
    }
    assert_eq!(lab.owner.active_operations()?, 0);
    eventually(Duration::from_secs(20), || {
        Ok(lab.member.active_operations()? == 0)
    })?;

    // The restarted target serves again, and its exposures and grants survived.
    succeed(&lab.remote(&["exec"], &["--", "true"])?)?;
    lab.shutdown()
}

fn ended_without_success(child: &mut Child, limit: Duration) -> TestResult {
    let deadline = Instant::now() + limit;
    while child.try_wait()?.is_none() {
        if Instant::now() > deadline {
            return Err("a session outlived the withdrawal of its authorization".into());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        !child.wait()?.success(),
        "a stopped session reported success"
    );
    Ok(())
}

#[test]
fn withdrawn_consent_or_membership_ends_running_sessions() -> TestResult {
    let lab = Lab::start("withdrawn", "1")?;
    let mut first = session(&lab)?;
    eventually(Duration::from_secs(30), || {
        Ok(lab.owner.active_operations()? == 1)
    })?;
    succeed(
        &lab.owner
            .run(&["service", "unexpose", "shell", &lab.space])?,
    )?;
    ended_without_success(&mut first, Duration::from_secs(20))?;

    succeed(&lab.owner.run(&["service", "expose", "shell", &lab.space])?)?;
    let mut second = session(&lab)?;
    eventually(Duration::from_secs(30), || {
        Ok(lab.owner.active_operations()? == 1)
    })?;
    succeed(
        &lab.owner
            .run(&["space", "member", "remove", &lab.space, lab.member.id()])?,
    )?;
    ended_without_success(&mut second, Duration::from_secs(20))?;
    eventually(Duration::from_secs(20), || {
        Ok(lab.owner.active_operations()? == 0)
    })?;
    lab.shutdown()
}
