//! The Phase 4 acceptance path: Agent `lead` on A delegates to Agent `worker` on B, which
//! fetches a file from A's root, streams progress, and publishes it as an artifact.

use std::fmt::Write as _;

use serde_json::Value;

use super::{
    daemon_fixture::TestResult,
    harness::{content, refused, succeed, text},
    lab::{AgentLab, INPUT_LEN, last, lines},
};

const REQUEST_ID: &str = "4a4a4a4a4a4a4a4a4a4a4a4a4a4a4a4a";

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut hex, byte| {
        let _written = write!(hex, "{byte:02x}");
        hex
    })
}

#[test]
fn a_delegated_task_fetches_from_its_creator_and_returns_an_artifact() -> TestResult {
    let lab = AgentLab::start("accept")?;
    let expected = hex(blake3::hash(&content(INPUT_LEN)).as_bytes());
    let instruction = format!(
        "delegate {} worker fetch {} docs /input.txt report.txt",
        lab.b.id(),
        lab.a.id()
    );
    let submitted = lab
        .a
        .run_ok(&["agent", "submit", "lead", "--text", &instruction, "--wait"])?;
    let details = last(&submitted)?;
    assert_eq!(text(&details, "/task/state"), "completed", "{details}");
    let result = text(&details, "/result/parts/0/content");
    assert!(
        result.starts_with("worker completed: fetched report.txt"),
        "{details}"
    );
    // `lead` republished the artifact it downloaded from `worker`, byte for byte.
    assert_eq!(text(&details, "/artifacts/0/name"), "report.txt");
    assert_eq!(text(&details, "/artifacts/0/blake3"), expected);

    // B's operator sees the task A created, attributed to `lead`.
    let tasks = last(&lab.b.run_ok(&["agent", "tasks", "worker"])?)?;
    let task = tasks
        .pointer("/tasks/0")
        .cloned()
        .ok_or_else(|| format!("worker has no task: {tasks}"))?;
    assert_eq!(text(&task, "/creator_endpoint_id"), lab.a.id());
    assert_eq!(text(&task, "/from_agent"), "lead");
    assert_eq!(text(&task, "/state"), "completed");
    let worker_task = text(&task, "/task_id");

    // A follows the finished task's retained events: progress, then the artifact.
    let watched = lab.remote(&["watch", "--agent", "worker", &worker_task])?;
    succeed(&watched)?;
    let events = lines(&watched)?;
    let kinds: Vec<String> = events.iter().map(|event| text(event, "/kind")).collect();
    assert!(kinds.iter().any(|kind| kind == "progress"), "{events:?}");
    assert!(kinds.iter().any(|kind| kind == "artifact"), "{events:?}");
    let summary = events.last().ok_or("watch printed nothing")?;
    assert_eq!(text(summary, "/state"), "completed");

    // A downloads the artifact from B and gets exactly its own file back.
    let output = lab.output_path("report.txt");
    let output_arg = output.to_str().ok_or("output path is not UTF-8")?;
    let artifact = [
        "artifact",
        "--agent",
        "worker",
        &worker_task,
        "report.txt",
        "--output",
        output_arg,
    ];
    let downloaded = last(&lab.remote(&artifact)?)?;
    assert_eq!(text(&downloaded, "/blake3"), expected);
    assert_eq!(std::fs::read(&output)?, content(INPUT_LEN));
    // An existing output file is never overwritten.
    refused(&lab.remote(&artifact)?, "exists")?;

    // The same task through the local API and through A's remote view agree.
    let remote = last(&lab.remote(&["task", "--agent", "worker", &worker_task])?)?;
    assert_eq!(remote.pointer("/artifacts"), details.pointer("/artifacts"));
    lab.shutdown()
}

#[test]
fn the_same_submit_creates_one_task_and_a_different_one_conflicts() -> TestResult {
    let lab = AgentLab::start("replay")?;
    let first = lab.submit_remote("echo once", Some(REQUEST_ID))?;
    let again = lab.submit_remote("echo once", Some(REQUEST_ID))?;
    assert_eq!(first, again);
    refused(
        &lab.remote(&[
            "submit",
            "worker",
            "--text",
            "echo twice",
            "--request-id",
            REQUEST_ID,
        ])?,
        "conflict",
    )?;
    let ended = lab.wait_ended_on_b(&first)?;
    assert_eq!(text(&ended, "/task/state"), "completed");
    assert_eq!(text(&ended, "/result/parts/0/content"), "once");
    let listed = last(&lab.b.run_ok(&["agent", "tasks"])?)?;
    let tasks = listed.pointer("/tasks").and_then(Value::as_array);
    assert_eq!(tasks.map(Vec::len), Some(1), "{listed}");

    // A task that asks for input waits for its creator's answer.
    let asked = lab.submit_remote("ask colour?", None)?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while text(
        &last(&lab.remote(&["task", "--agent", "worker", &asked])?)?,
        "/task/state",
    ) != "waiting"
    {
        if std::time::Instant::now() >= deadline {
            return Err("the asking task never waited for input".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let sent = lab.remote(&["send", "--agent", "worker", &asked, "--text", "blue"])?;
    succeed(&sent)?;
    let answered = lab.wait_ended_on_b(&asked)?;
    assert_eq!(text(&answered, "/result/parts/0/content"), "colour? blue");
    lab.shutdown()
}
