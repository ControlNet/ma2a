//! The Phase 5 acceptance path: discovery, version selection, every interaction, caller-side
//! replay, cancellation, saturation, and denial, all through one service framework.

use std::time::Duration;

use serde_json::Value;

use super::{
    daemon_fixture::TestResult,
    harness::{Node, content, join, refused, succeed, text},
    lab::{EcoLab, SERVICE, exited, hosted, interrupt, wait_running},
};

const REQUEST_ID: &str = "5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e";

fn named<'a>(page: &'a Value, name: &str) -> Option<&'a Value> {
    page.pointer("/services")?
        .as_array()?
        .iter()
        .find(|service| text(service, "/service") == name)
}

fn versions(descriptor: &Value) -> Vec<u64> {
    descriptor
        .pointer("/versions")
        .and_then(Value::as_array)
        .map(|versions| {
            versions
                .iter()
                .filter_map(|version| version.get("version").and_then(Value::as_u64))
                .collect()
        })
        .unwrap_or_default()
}

/// How many calls of `version` the hosting Runtime admitted.
fn admitted(view: &Value, version: u64) -> u64 {
    view.pointer("/calls")
        .and_then(Value::as_array)
        .and_then(|calls| {
            calls
                .iter()
                .find(|call| call.get("version").and_then(Value::as_u64) == Some(version))
        })
        .and_then(|call| call.get("count").and_then(Value::as_u64))
        .unwrap_or(0)
}

fn stdout(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn discovery_versions_interactions_and_replay_share_one_framework() -> TestResult {
    let lab = EcoLab::start("eco-accept")?;

    // Discovery is per Endpoint: the hosted service and the built-in Exec, both exposed to
    // `eco`, with their versions.
    let page = lab.discover()?;
    let example = named(&page, SERVICE).ok_or_else(|| format!("no {SERVICE}: {page}"))?;
    assert_eq!(example.get("builtin"), Some(&Value::Bool(false)), "{page}");
    assert_eq!(example.get("available"), Some(&Value::Bool(true)), "{page}");
    assert_eq!(versions(example), [1, 2], "{page}");
    let exec = named(&page, "exec").ok_or_else(|| format!("no exec: {page}"))?;
    assert_eq!(exec.get("builtin"), Some(&Value::Bool(true)), "{page}");

    // Without --version the caller speaks the highest version listing the operation.
    let echoed = lab.call(&[SERVICE, "echo", "--body", "hello"])?;
    succeed(&echoed)?;
    assert_eq!(stdout(&echoed), "hello");
    let upper = lab.call(&[SERVICE, "upper", "--version", "1", "--body", "hello"])?;
    succeed(&upper)?;
    assert_eq!(stdout(&upper), "HELLO");
    let counted = lab.call(&[SERVICE, "count", "--body", "3"])?;
    succeed(&counted)?;
    assert_eq!(stdout(&counted), "1\n2\n3\n3");

    // A version the service does not speak, and an operation its version lacks.
    refused(
        &lab.call(&[SERVICE, "echo", "--version", "3"])?,
        "version_mismatch",
    )?;
    refused(
        &lab.call(&[SERVICE, "count", "--version", "1", "--body", "3"])?,
        "invalid_input",
    )?;
    // The service itself refuses a body it cannot serve, before any effect.
    refused(
        &lab.call(&[SERVICE, "count", "--body", "0"])?,
        "invalid_input",
    )?;

    // Bytes both ways, larger than the relay buffers.
    let input = content(200_000);
    let piped = lab.piped(&[SERVICE, "pipe", "--stdin"], &input)?;
    succeed(&piped)?;
    let mut expected = input;
    expected.extend_from_slice(b"200000");
    assert!(
        piped.stdout == expected,
        "pipe returned {} bytes",
        piped.stdout.len()
    );

    // Caller-side replay: the same RequestId replays without a second call, and a different
    // call under it conflicts.
    let once = lab.call(&[
        SERVICE,
        "echo",
        "--request-id",
        REQUEST_ID,
        "--body",
        "once",
    ])?;
    succeed(&once)?;
    let before = admitted(&hosted(&lab.a)?, 2);
    let again = lab.call(&[
        SERVICE,
        "echo",
        "--request-id",
        REQUEST_ID,
        "--body",
        "once",
    ])?;
    succeed(&again)?;
    assert_eq!(stdout(&again), "once");
    assert_eq!(
        admitted(&hosted(&lab.a)?, 2),
        before,
        "the replay reached the service"
    );
    refused(
        &lab.call(&[
            SERVICE,
            "echo",
            "--request-id",
            REQUEST_ID,
            "--body",
            "other",
        ])?,
        "conflict",
    )?;
    assert!(admitted(&hosted(&lab.a)?, 1) >= 1);

    // Built-in services still run through the same plane.
    succeed(
        &lab.b
            .run(&["exec", "--endpoint", lab.a.id(), "--", "true"])?,
    )?;
    lab.shutdown()
}

#[test]
fn cancellation_saturation_and_denial_are_exact() -> TestResult {
    let lab = EcoLab::start("eco-limits")?;

    // `example` serves two calls at once; a third is refused before any effect.
    let first = lab.spawn_call(&[SERVICE, "hold"])?;
    wait_running(&lab.a, 1)?;
    let second = lab.spawn_call(&[SERVICE, "hold"])?;
    wait_running(&lab.a, 2)?;
    refused(&lab.call(&[SERVICE, "hold"])?, "concurrency_exceeded")?;

    // Ctrl-C cancels a call: the service sees `cancel` and ends it `cancelled`.
    interrupt(&first)?;
    refused(&first.wait_with_output()?, "cancelled")?;
    wait_running(&lab.a, 1)?;
    interrupt(&second)?;
    refused(&second.wait_with_output()?, "cancelled")?;
    wait_running(&lab.a, 0)?;

    // A stream cancelled midway stops where it was.
    let counting = lab.spawn_call(&[SERVICE, "count", "--body", "1000"])?;
    wait_running(&lab.a, 1)?;
    std::thread::sleep(Duration::from_millis(300));
    interrupt(&counting)?;
    let counted = counting.wait_with_output()?;
    refused(&counted, "cancelled")?;
    let lines = stdout(&counted);
    assert!(
        lines.starts_with("1\n") && !lines.contains("1000\n"),
        "{lines}"
    );
    wait_running(&lab.a, 0)?;

    // A cancelled call ends even while the stdin it reads stays open.
    let mut piping = lab.spawn_call_reading(&[SERVICE, "pipe", "--stdin"])?;
    wait_running(&lab.a, 1)?;
    interrupt(&piping)?;
    let status = exited(&mut piping, Duration::from_secs(30))?;
    assert!(!status.success(), "{status}");
    wait_running(&lab.a, 0)?;

    // A service this Endpoint may not use answers exactly like one that does not exist.
    let c = Node::start("eco-limits-c")?;
    let space = join(&lab.a, &c, "eco")?;
    let ungranted = c.run(&["service", "call", "--endpoint", lab.a.id(), SERVICE, "echo"])?;
    refused(&ungranted, "unknown_service")?;
    let versioned = [SERVICE, "echo", "--version", "1"];
    let mut arguments = vec!["service", "call", "--endpoint", lab.a.id()];
    arguments.extend(versioned);
    refused(&c.run(&arguments)?, "unknown_service")?;
    refused(
        &lab.call(&["absent", "echo", "--version", "1"])?,
        "unknown_service",
    )?;
    refused(&lab.call(&["absent", "echo"])?, "unknown_service")?;
    let discovered = c.run(&["service", "discover", "--endpoint", lab.a.id()])?;
    succeed(&discovered)?;
    let page: Value = serde_json::from_slice(&discovered.stdout)?;
    assert!(named(&page, SERVICE).is_none(), "{page}");
    assert!(named(&page, "exec").is_none(), "{page}");

    // A grant in another Space never combines with the exposure in `eco`.
    let other = join(&lab.a, &c, "eco-other")?;
    lab.a
        .run_ok(&["space", "grant", &other, c.id(), "--service"])?;
    refused(&c.run(&arguments)?, "unknown_service")?;

    // A removed member loses discovery at once.
    for space in [&space, &other] {
        succeed(&lab.a.run(&["space", "member", "remove", space, c.id()])?)?;
    }
    let removed = c.run(&["service", "discover", "--endpoint", lab.a.id()])?;
    refused(&removed, "unauthorized")?;

    // An Endpoint sharing no Space cannot even locate the target.
    let outsider = Node::start("eco-limits-outsider")?;
    let probed = outsider.run(&["service", "discover", "--endpoint", lab.a.id()])?;
    refused(&probed, "unavailable")?;
    outsider.shutdown()?;
    c.shutdown()?;
    lab.shutdown()
}
