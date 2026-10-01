//! What a caller cannot do to an Agent: reach one it was not given, use another
//! creator's task, combine two Spaces, exceed the limits, or keep its tasks after
//! revocation. A claimed `from_agent` is only ever attribution.

use serde_json::Value;

use super::{
    daemon_fixture::{TestResult, TestValue},
    harness::{Node, join, refused, succeed, text},
    lab::{AgentLab, last, wait_ready},
};

/// Non-terminal tasks one creator may hold on one Runtime.
const PER_CREATOR: usize = 16;

/// Runs `ma2a agent --endpoint B ARGS…` on `node`.
fn remote_from(node: &Node, lab: &AgentLab, arguments: &[&str]) -> TestValue<std::process::Output> {
    let mut command = vec!["agent", "--endpoint", lab.b.id()];
    command.extend(arguments);
    node.run(&command)
}

/// Runs a command on `node` until B answers anything but `unavailable`: B accepts a new
/// member's streams only once the Space change has replicated to it.
fn settled(node: &Node, lab: &AgentLab, arguments: &[&str]) -> TestValue<std::process::Output> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let output = remote_from(node, lab, arguments)?;
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !stderr.starts_with("unavailable") || std::time::Instant::now() >= deadline {
            return Ok(output);
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

#[test]
fn unknown_stopped_foreign_and_cross_space_agents_are_refused() -> TestResult {
    let lab = AgentLab::start("adv-deny")?;
    // B exposes only `worker`; any other name is unknown, whether or not it exists.
    refused(
        &lab.remote(&["submit", "nobody", "--text", "x"])?,
        "unknown_agent",
    )?;
    let discovered = last(&lab.remote(&["discover"])?)?;
    let agents = discovered.pointer("/agents").and_then(Value::as_array);
    assert_eq!(agents.map(Vec::len), Some(1), "{discovered}");

    // A stopped Agent takes nothing new, and takes tasks again once started.
    lab.b.run_ok(&["agent", "stop", "worker"])?;
    refused(
        &lab.remote(&["submit", "worker", "--text", "echo x"])?,
        "agent_unavailable",
    )?;
    lab.b.run_ok(&["agent", "start", "worker"])?;
    wait_ready(&lab.b, "worker")?;
    let owned = lab.submit_remote("sleep 30000", None)?;

    // C is a member of the Space without the Agent grant; claiming to be an Agent changes
    // nothing.
    let c = Node::start("adv-deny-c")?;
    join(&lab.a, &c, "lab")?;
    let forged = [
        "submit",
        "worker",
        "--text",
        "echo x",
        "--from-agent",
        "lead",
    ];
    refused(&settled(&c, &lab, &forged)?, "unauthorized")?;

    // With the grant, C uses `worker` as itself: A's task does not exist for it, and its
    // own task is attributed to the Agent it named, but created by C.
    lab.a
        .run_ok(&["space", "grant", &lab.space, c.id(), "--agent"])?;
    let task = ["task", "--agent", "worker", &owned];
    refused(&remote_from(&c, &lab, &task)?, "unknown_task")?;
    let cancel = ["cancel", "--agent", "worker", &owned];
    refused(&remote_from(&c, &lab, &cancel)?, "unknown_task")?;
    let submitted = last(&remote_from(&c, &lab, &forged)?)?;
    let created = text(&submitted, "/task/task_id");
    let ended = lab.wait_ended_on_b(&created)?;
    assert_eq!(text(&ended, "/task/creator_endpoint_id"), c.id(), "{ended}");
    assert_eq!(text(&ended, "/task/from_agent"), "lead", "{ended}");
    refused(
        &lab.remote(&["task", "--agent", "worker", &created])?,
        "unknown_task",
    )?;

    // A grant in a second Space never combines with the exposure in the first.
    let other = join(&lab.a, &c, "other")?;
    join(&lab.a, &lab.b, "other")?;
    lab.a
        .run_ok(&["space", "grant", &other, c.id(), "--agent"])?;
    lab.a.run_ok(&["space", "grant", &lab.space, c.id()])?;
    // Until C's control state converges, C may still pass admission and see no Agent.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let denied = remote_from(&c, &lab, &["submit", "worker", "--text", "echo x"])?;
        let stderr = String::from_utf8_lossy(&denied.stderr);
        if !denied.status.success() && stderr.starts_with("unauthorized") {
            break;
        }
        if std::time::Instant::now() >= deadline {
            return Err(format!("the revoked grant still admitted C: {stderr}").into());
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    // A's own task was never touched.
    let still = last(&lab.remote(&["task", "--agent", "worker", &owned])?)?;
    assert_eq!(text(&still, "/task/state"), "running", "{still}");
    c.shutdown()?;
    lab.shutdown()
}

#[test]
fn the_creator_limit_holds_and_revocation_cancels_every_task() -> TestResult {
    let lab = AgentLab::start("adv-limit")?;
    let tasks = (0..PER_CREATOR)
        .map(|_| lab.submit_remote("sleep 60000", None))
        .collect::<TestValue<Vec<_>>>()?;
    refused(
        &lab.remote(&["submit", "worker", "--text", "echo x"])?,
        "concurrency_exceeded",
    )?;
    let listed = last(&lab.b.run_ok(&["agent", "tasks", "worker"])?)?;
    let count = listed
        .pointer("/tasks")
        .and_then(Value::as_array)
        .map(Vec::len);
    assert_eq!(count, Some(PER_CREATOR), "{listed}");

    // Removing A's grant cancels everything A created, as `unauthorized`.
    succeed(&lab.a.run(&["space", "grant", &lab.space, lab.a.id()])?)?;
    for task in &tasks {
        let ended = lab.wait_ended_on_b(task)?;
        assert_eq!(text(&ended, "/task/state"), "cancelled", "{ended}");
        assert_eq!(text(&ended, "/task/reason"), "unauthorized", "{ended}");
    }
    refused(
        &lab.remote(&["submit", "worker", "--text", "echo x"])?,
        "unauthorized",
    )?;
    lab.shutdown()
}
