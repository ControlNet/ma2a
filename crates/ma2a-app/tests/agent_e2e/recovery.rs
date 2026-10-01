//! Restarts and crashes: a watch reconnects, a lost instance fails its tasks and restarts,
//! and a Runtime that crashes at any fault point finishes every task exactly once.

use serde_json::Value;

use super::{
    daemon_fixture::TestResult,
    harness::{succeed, text},
    lab::{AgentLab, last, lines, wait_ready},
};

#[test]
fn a_watch_follows_its_task_across_a_target_restart() -> TestResult {
    let mut lab = AgentLab::start("rewatch")?;
    let task = lab.submit_remote("sleep 4000", None)?;
    let watching = lab.spawn_remote(&["watch", "--agent", "worker", &task])?;
    std::thread::sleep(std::time::Duration::from_millis(800));
    lab.b.restart()?;
    let watched = watching.wait_with_output()?;
    succeed(&watched)?;
    let events = lines(&watched)?;
    let summary = events.last().ok_or("watch printed nothing")?;
    assert_eq!(text(summary, "/state"), "completed", "{events:?}");
    // Every event arrives once and in order, across the reconnection.
    let seqs: Vec<u64> = events
        .iter()
        .filter_map(|event| event.get("seq").and_then(Value::as_u64))
        .collect();
    assert!(seqs.len() > 3, "{events:?}");
    assert!(
        seqs.is_sorted_by(|before, after| before < after),
        "{seqs:?}"
    );
    lab.shutdown()
}

#[test]
fn a_crashed_agent_fails_its_tasks_and_is_restarted() -> TestResult {
    let lab = AgentLab::start("agentlost")?;
    let crashed = last(
        &lab.b
            .run_ok(&["agent", "submit", "worker", "--text", "crash", "--wait"])?,
    )?;
    assert_eq!(text(&crashed, "/task/state"), "failed", "{crashed}");
    assert_eq!(text(&crashed, "/task/reason"), "agent_lost", "{crashed}");
    let echoed =
        last(
            &lab.b
                .run_ok(&["agent", "submit", "worker", "--text", "echo back", "--wait"])?,
        )?;
    assert_eq!(text(&echoed, "/task/state"), "completed", "{echoed}");
    let listed = last(&lab.b.run_ok(&["agent", "list"])?)?;
    let generation = listed
        .pointer("/agents/0/generation")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    assert!(generation >= 2, "{listed}");
    lab.shutdown()
}

#[test]
fn a_crashed_runtime_resumes_its_running_task() -> TestResult {
    let mut lab = AgentLab::start("resume")?;
    let submitted = last(
        &lab.b
            .run_ok(&["agent", "submit", "worker", "--text", "sleep 2500"])?,
    )?;
    let task = text(&submitted, "/task/task_id");
    std::thread::sleep(std::time::Duration::from_millis(500));
    lab.b.crash_restart(&[])?;
    let ended = lab.wait_ended_on_b(&task)?;
    assert_eq!(text(&ended, "/task/state"), "completed", "{ended}");
    lab.shutdown()
}

/// Crashes B at `point` while `worker` fetches from A, then checks the task still
/// completes once, with its artifact.
///
/// `label` stays short: supervisor sockets live under the state directory, and a Unix
/// socket path has at most 103 bytes.
fn crash_at((label, point): (&str, &str)) -> TestResult {
    let mut lab = AgentLab::start(label)?;
    lab.b.restart_with(&[("MA2A_TEST_AGENT_FAULT", point)])?;
    wait_ready(&lab.b, "worker")?;
    let instruction = format!("fetch {} docs /input.txt fetched.bin", lab.a.id());
    // The submit itself may be the operation the daemon aborts in.
    let _submitted = lab
        .b
        .run(&["agent", "submit", "worker", "--text", &instruction]);
    lab.b.recover_from_fault()?;
    let listed = last(&lab.b.run_ok(&["agent", "tasks", "worker"])?)?;
    let tasks = listed
        .pointer("/tasks")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(tasks.len(), 1, "{point}: {listed}");
    let task = text(tasks.first().ok_or("no task")?, "/task_id");
    let ended = lab.wait_ended_on_b(&task)?;
    assert_eq!(text(&ended, "/task/state"), "completed", "{point}: {ended}");
    let artifacts = ended
        .pointer("/artifacts")
        .and_then(Value::as_array)
        .map(Vec::len);
    assert_eq!(artifacts, Some(1), "{point}: {ended}");
    lab.shutdown()
}

#[test]
fn a_crash_after_the_task_is_created_still_runs_it_once() -> TestResult {
    crash_at(("f-created", "after_task_created"))
}

#[test]
fn a_crash_after_assignment_redelivers_the_task() -> TestResult {
    crash_at(("f-assigned", "after_assigned"))
}

#[test]
fn a_crash_after_a_tool_call_is_recorded_runs_it_once() -> TestResult {
    crash_at(("f-recorded", "after_tool_recorded"))
}

#[test]
fn a_crash_before_a_tool_result_is_stored_answers_it_once() -> TestResult {
    crash_at(("f-result", "before_tool_result"))
}
