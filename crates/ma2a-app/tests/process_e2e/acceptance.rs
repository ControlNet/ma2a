//! The Phase 3 acceptance path: start, disconnect, continue, read, attach, end, retry.

use std::time::Duration;

use rustix::process::Signal;
use serde_json::Value;

use super::{
    daemon_fixture::TestResult,
    harness::{refused, succeed, text},
    lab::{ProcessLab, TICKER, first_line, signal},
};

const REQUEST_ID: &str = "0123456789abcdef0123456789abcdef";

#[test]
fn a_process_outlives_its_caller_and_is_read_attached_ended_and_archived() -> TestResult {
    let lab = ProcessLab::start("accept")?;
    let start = [
        "--request-id",
        REQUEST_ID,
        "--name",
        "ticker",
        "--label",
        "team=ops",
        "--env",
        "TICKER_TOKEN=not-for-display",
        "--",
        "sh",
        "-c",
        TICKER,
    ];
    let id = lab.start_process(&start)?;
    // The starting command has exited: the Process keeps running without any caller.
    lab.wait_state(&id, "running")?;

    // The same request again answers the same Process; a different body is a conflict.
    // The caller's daemon replays its recorded outcome, which carries no `existing` flag;
    // the target's own binding answers `existing` only to a retry that reaches it.
    let mut retry = vec!["start"];
    retry.extend(start);
    let retried = lab.json(&retry)?;
    assert_eq!(text(&retried, "/process_id"), id);
    assert!(
        matches!(retried.pointer("/existing"), None | Some(Value::Bool(true))),
        "{retried}"
    );
    let conflicting = lab.remote(&["start", "--request-id", REQUEST_ID, "--", "true"])?;
    refused(&conflicting, "conflict")?;
    let listed = lab.json(&["list"])?;
    let processes = listed.pointer("/processes").and_then(Value::as_array);
    assert_eq!(processes.map(Vec::len), Some(1), "{listed}");

    // The host's operator sees the creator and the environment names, never their values.
    let local = lab.owner.run_ok(&["process", "inspect", &id])?;
    let local_text = String::from_utf8(local.stdout)?;
    assert!(!local_text.contains("not-for-display"), "{local_text}");
    let local: Value = serde_json::from_str(&local_text)?;
    assert_eq!(
        text(&local, "/process/creator_endpoint_id"),
        lab.member.id()
    );
    assert_eq!(
        local.pointer("/start/env_names"),
        Some(&serde_json::json!(["TICKER_TOKEN"]))
    );
    let inspected = lab.json(&["inspect", &id])?;
    assert_eq!(text(&inspected, "/process/name"), "ticker");
    assert_eq!(text(&inspected, "/start/labels/0/value"), "ops");

    std::thread::sleep(Duration::from_millis(500));
    let logs = lab.remote(&["logs", &id])?;
    succeed(&logs)?;
    assert!(String::from_utf8(logs.stdout)?.starts_with("tick 0\n"));
    assert!(String::from_utf8(logs.stderr)?.contains("next cursor"));

    // Attach to live output, then detach with Ctrl-C: the Process keeps running.
    let mut attached = lab.spawn(&["attach", &id])?;
    let line = first_line(&mut attached)?;
    if !line.starts_with("tick ") {
        let output = attached.wait_with_output()?;
        return Err(format!("attach printed {line:?}: {output:?}").into());
    }
    signal(&attached, Signal::INT)?;
    let detached = attached.wait_with_output()?;
    assert!(detached.status.success(), "{detached:?}");
    assert!(String::from_utf8(detached.stderr)?.contains("detached"));
    assert_eq!(
        text(&lab.json(&["inspect", &id])?, "/process/state"),
        "running"
    );

    let ended = lab.json(&["terminate", &id, "--grace", "1s"])?;
    assert_eq!(text(&ended, "/state"), "terminated");
    assert_eq!(text(&ended, "/reason"), "requested");
    let deleted = lab.json(&["delete", &id])?;
    assert_eq!(deleted.pointer("/deleted"), Some(&Value::Bool(true)));
    let listed = lab.json(&["list"])?;
    assert_eq!(listed.pointer("/processes"), Some(&serde_json::json!([])));
    // The owner still keeps the tombstone, so the RequestId stays bound.
    let tombstones = lab.owner.run_ok(&["process", "list"])?;
    let tombstones: Value = serde_json::from_slice(&tombstones.stdout)?;
    assert_eq!(
        tombstones.pointer("/processes/0/archived"),
        Some(&Value::Bool(true))
    );
    lab.shutdown()
}

#[test]
fn stdin_reaches_the_process_through_attach_and_stdin() -> TestResult {
    let lab = ProcessLab::start("stdin")?;
    let echo = [
        "--stdin",
        "--",
        "sh",
        "-c",
        "read line; echo got:$line; exit 7",
    ];
    let id = lab.start_process(&echo)?;
    let attach = [
        "process",
        "--endpoint",
        lab.owner.id(),
        "attach",
        &id,
        "--stdin",
        "--from",
        "start",
    ];
    let attached = lab.member.piped(&attach, b"hello\n")?;
    // The attachment ends with the Process, and exits with its status.
    assert_eq!(attached.status.code(), Some(7), "{attached:?}");
    assert_eq!(attached.stdout, b"got:hello\n");

    let id = lab.start_process(&["--stdin", "--", "cat"])?;
    lab.wait_state(&id, "running")?;
    let write = ["process", "--endpoint", lab.owner.id(), "stdin", &id];
    let written = lab.member.piped(&write, b"abc")?;
    succeed(&written)?;
    let written: Value = serde_json::from_slice(&written.stdout)?;
    assert_eq!(
        written,
        serde_json::json!({"written": 3, "stdin_closed": true})
    );
    lab.wait_state(&id, "exited")?;
    let logs = lab.remote(&["logs", &id])?;
    assert_eq!(logs.stdout, b"abc");
    // Its stdin is closed for good now.
    refused(&lab.member.piped(&write, b"more")?, "not_running")?;
    lab.shutdown()
}
