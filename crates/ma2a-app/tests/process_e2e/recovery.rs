//! Daemon restarts and crashes: Processes are re-adopted through the ownership proof, and a
//! crash at any point of a start never produces a second Process.

use serde_json::Value;

use super::{
    daemon_fixture::TestResult,
    harness::{refused, text},
    lab::{LONG_TICKER, ProcessLab},
};

#[test]
fn a_restarted_or_crashed_daemon_readopts_its_running_processes() -> TestResult {
    let mut lab = ProcessLab::start("readopt")?;
    let id = lab.start_process(&["--", "sh", "-c", LONG_TICKER])?;
    let running = lab.wait_state(&id, "running")?;
    let started_at = running.pointer("/process/started_at_ms").cloned();

    lab.owner.restart()?;
    let readopted = lab.wait_state(&id, "running")?;
    assert_eq!(
        readopted.pointer("/process/started_at_ms").cloned(),
        started_at
    );

    lab.owner.crash_restart(&[])?;
    let readopted = lab.wait_state(&id, "running")?;
    assert_eq!(
        readopted.pointer("/process/started_at_ms").cloned(),
        started_at
    );
    // Output written while no daemon was running is still there, in order.
    let logs = String::from_utf8(lab.remote(&["logs", &id])?.stdout)?;
    let ticks: Vec<&str> = logs.lines().collect();
    assert!(ticks.len() > 5, "{logs}");
    assert!(
        ticks
            .iter()
            .enumerate()
            .all(|(n, line)| *line == format!("tick {n}"))
    );
    let ended = lab.json(&["terminate", &id, "--grace", "500ms"])?;
    assert_eq!(text(&ended, "/state"), "terminated");
    lab.shutdown()
}

/// Crashes the owner at `point` during one start, then checks the one Process it left.
fn crash_during_start(lab: &mut ProcessLab, (point, request_id): (&str, &str)) -> TestResult {
    lab.owner
        .restart_with(&[("MA2A_TEST_PROCESS_FAULT", point)])?;
    let start = [
        "start",
        "--request-id",
        request_id,
        "--",
        "sh",
        "-c",
        LONG_TICKER,
    ];
    // The owner is brought back while the caller still waits: a caller only learns of the
    // crash when its connection times out, and the child must not end before re-adoption.
    let waiting = lab.spawn(&start)?;
    lab.owner.recover_from_fault()?;
    // The caller cannot know what happened on the target.
    refused(&waiting.wait_with_output()?, "outcome_unknown")?;
    // Retrying answers the recorded unknown outcome instead of starting anything.
    refused(&lab.remote(&start)?, "outcome_unknown")?;
    let listed = lab.json(&["list"])?;
    let bound: Vec<&Value> = listed
        .pointer("/processes")
        .and_then(Value::as_array)
        .map(|processes| {
            processes
                .iter()
                .filter(|process| text(process, "/request_id") == request_id)
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(bound.len(), 1, "{point}: {listed}");
    let id = text(bound.first().ok_or("no Process")?, "/process_id");
    match point {
        // `go` was never sent, so no child ever existed.
        "after_created" | "after_launch" => {
            let failed = lab.wait_state(&id, "failed")?;
            assert_eq!(text(&failed, "/process/reason"), "interrupted", "{point}");
        }
        _ => {
            lab.wait_state(&id, "running")?;
            lab.json(&["terminate", &id, "--grace", "500ms"])?;
        }
    }
    Ok(())
}

/// Runs one crash point in a lab of its own: every point costs a caller connection timeout.
fn crash_point(point: &str, request_id: &str) -> TestResult {
    let mut lab = ProcessLab::start(&format!("fault-{}", point.replace('_', "-")))?;
    crash_during_start(&mut lab, (point, request_id))?;
    lab.shutdown()
}

#[test]
fn a_crash_after_the_binding_leaves_one_failed_process() -> TestResult {
    crash_point("after_created", "0000000000000000000000000000c001")
}

#[test]
fn a_crash_after_the_launch_leaves_one_failed_process() -> TestResult {
    crash_point("after_launch", "0000000000000000000000000000c002")
}

#[test]
fn a_crash_after_go_leaves_one_running_process() -> TestResult {
    crash_point("after_go", "0000000000000000000000000000c003")
}

#[test]
fn a_crash_before_the_response_leaves_one_running_process() -> TestResult {
    crash_point("before_response", "0000000000000000000000000000c004")
}
