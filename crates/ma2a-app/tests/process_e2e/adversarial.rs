//! Adversarial Phase 3 cases: lost supervisors, output overflow, broken attachments,
//! strangers, withdrawn consent, and limits.

use rustix::process::Signal;
use serde_json::Value;

use super::{
    daemon_fixture::TestResult,
    harness::{Node, join, refused, succeed, text},
    lab::{ProcessLab, TICKER, first_line, signal},
};

/// The supervisors a daemon launched: `ma2a __process-supervisor` children of `daemon`.
#[cfg(target_os = "linux")]
fn supervisors_of(daemon: u32) -> Vec<i32> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().to_str()?.parse::<i32>().ok())
        .filter(|pid| {
            let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
            let parent = stat
                .rsplit_once(')')
                .and_then(|(_, rest)| rest.split_whitespace().nth(1))
                .and_then(|ppid| ppid.parse::<u32>().ok());
            let command = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
            parent == Some(daemon)
                && command
                    .split(|byte| *byte == 0)
                    .any(|argument| argument == b"__process-supervisor")
        })
        .collect()
}

#[cfg(target_os = "linux")]
#[test]
fn a_lost_supervisor_makes_its_process_lost_and_never_running() -> TestResult {
    let mut lab = ProcessLab::start("lost")?;
    let id = lab.start_process(&["--", "sh", "-c", TICKER])?;
    lab.wait_state(&id, "running")?;
    let daemon = lab.owner.daemon_pid().ok_or("no daemon")?;
    let supervisors = supervisors_of(daemon);
    assert_eq!(supervisors.len(), 1, "{supervisors:?}");
    for pid in supervisors {
        let pid = rustix::process::Pid::from_raw(pid).ok_or("no pid")?;
        rustix::process::kill_process(pid, Signal::KILL)?;
    }
    // Without its supervisor nothing proves the child's fate: never `running`, never a PID.
    let lost = lab.wait_state(&id, "lost")?;
    assert_eq!(text(&lost, "/process/reason"), "supervisor_lost");

    // A supervisor that dies after its child ended but before recording the end.
    lab.owner
        .restart_with(&[("MA2A_TEST_SUPERVISOR_FAULT", "before_status")])?;
    let id = lab.start_process(&["--", "sh", "-c", "echo bye"])?;
    let lost = lab.wait_state(&id, "lost")?;
    assert_eq!(text(&lost, "/process/reason"), "supervisor_lost");
    assert_eq!(lab.remote(&["logs", &id])?.stdout, b"bye\n");
    lab.owner.restart()?;
    lab.shutdown()
}

#[test]
fn overflowing_output_is_bounded_and_a_broken_attachment_detaches_only() -> TestResult {
    let lab = ProcessLab::start("overflow")?;
    let flood = "head -c 3000000 /dev/zero | tr '\\0' a; echo; echo end";
    let id = lab.start_process(&["--log-limit", "1048576", "--", "sh", "-c", flood])?;
    let ended = lab.wait_state(&id, "exited")?;
    let first = ended
        .pointer("/log_first_available")
        .and_then(Value::as_u64);
    let end = ended.pointer("/log_end").and_then(Value::as_u64);
    assert!(first > Some(0) && end >= Some(3_000_005), "{ended}");
    let logs = lab.remote(&["logs", &id, "--from", "0"])?;
    succeed(&logs)?;
    assert!(String::from_utf8(logs.stderr)?.contains("older output was dropped"));
    assert!(logs.stdout.ends_with(b"a\nend\n"));
    assert!(
        logs.stdout.len() <= 2 * 1_048_576,
        "{} retained",
        logs.stdout.len()
    );

    // An attachment killed outright only detaches; its reader slot is released.
    let id = lab.start_process(&["--", "sh", "-c", TICKER])?;
    lab.wait_state(&id, "running")?;
    let mut broken = lab.spawn(&["attach", &id])?;
    first_line(&mut broken)?;
    signal(&broken, Signal::KILL)?;
    broken.wait()?;
    let mut again = lab.spawn(&["attach", &id])?;
    assert!(first_line(&mut again)?.starts_with("tick "));
    signal(&again, Signal::INT)?;
    assert!(again.wait()?.success());
    assert_eq!(
        text(&lab.json(&["inspect", &id])?, "/process/state"),
        "running"
    );
    lab.shutdown()
}

#[test]
fn strangers_see_nothing_limits_hold_and_withdrawn_consent_ends_processes() -> TestResult {
    let lab = ProcessLab::start("consent")?;
    let stranger = Node::start("consent-stranger")?;
    join(&lab.owner, &stranger, "lab")?;
    lab.owner
        .run_ok(&["space", "grant", &lab.space, stranger.id(), "--process"])?;

    let mut ids = Vec::new();
    for _ in 0..8 {
        ids.push(lab.start_process(&["--", "sleep", "30"])?);
    }
    refused(
        &lab.remote(&["start", "--", "sleep", "30"])?,
        "concurrency_exceeded",
    )?;
    let first = ids.first().ok_or("no Process")?.clone();
    lab.wait_state(&first, "running")?;
    // Another member of the same Space cannot see or touch the member's Processes.
    let seen = lab.remote_from(&stranger, &["list"])?;
    succeed(&seen)?;
    let seen: Value = serde_json::from_slice(&seen.stdout)?;
    assert_eq!(seen.pointer("/processes"), Some(&serde_json::json!([])));
    for operation in [["inspect", first.as_str()], ["terminate", first.as_str()]] {
        refused(&lab.remote_from(&stranger, &operation)?, "unknown_process")?;
    }

    // Withdrawing consent ends every Process it allowed.
    lab.owner
        .run_ok(&["service", "unexpose", "process", &lab.space])?;
    for id in &ids {
        let ended = lab.wait_local_state(id, "terminated")?;
        assert_eq!(text(&ended, "/process/reason"), "unauthorized");
    }
    refused(&lab.remote(&["start", "--", "true"])?, "unauthorized")?;
    stranger.shutdown()?;
    lab.shutdown()
}
