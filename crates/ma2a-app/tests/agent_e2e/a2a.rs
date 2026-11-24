//! The A2A v1.0 adapter end to end: an A2A JSON-RPC client sends, streams, gets, lists,
//! and cancels against A's own Agent and B's Agent through A, and fetches artifacts.

use std::{
    io::{BufRead as _, BufReader},
    process::{Child, Stdio},
};

use serde_json::{Value, json};

use super::{
    daemon_fixture::{TestResult, TestValue},
    harness::{content, text},
    http_client::{Answer, request},
    lab::{AgentLab, INPUT_LEN},
};

/// One running adapter on A and how to call it.
struct Adapter {
    child: Child,
    url: String,
    token: String,
    token_file: String,
}

impl Adapter {
    fn start(lab: &AgentLab) -> TestValue<Self> {
        let mut child = lab
            .a
            .command(&["a2a", "serve", "--listen", "127.0.0.1:0"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let mut line = String::new();
        BufReader::new(child.stdout.take().ok_or("no adapter stdout")?).read_line(&mut line)?;
        let ready: Value = serde_json::from_str(&line)?;
        let token_file = text(&ready, "/token_file");
        let mode =
            std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&token_file)?.permissions());
        assert_eq!(mode & 0o777, 0o600, "the token file is private");
        Ok(Self {
            child,
            url: text(&ready, "/url"),
            token: std::fs::read_to_string(&token_file)?,
            token_file,
        })
    }

    fn get(&self, path: &str) -> TestValue<Answer> {
        let bearer = format!("Bearer {}", self.token);
        request(
            ("GET", &format!("{}{path}", self.url)),
            &[("Authorization", &bearer)],
            None,
        )
    }

    /// Calls one JSON-RPC method on the route at `path`; answers the whole response.
    fn call(&self, (path, method): (&str, &str), params: &Value) -> TestValue<Answer> {
        let bearer = format!("Bearer {}", self.token);
        let body = json!({"jsonrpc": "2.0", "id": method, "method": method, "params": params});
        request(
            ("POST", &format!("{}{path}", self.url)),
            &[("Authorization", &bearer), ("A2A-Version", "1.0")],
            Some(body.to_string().as_bytes()),
        )
    }

    /// Calls one method and answers its `result`, failing on a JSON-RPC error.
    fn result(&self, (path, method): (&str, &str), params: &Value) -> TestValue<Value> {
        let answer = self.call((path, method), params)?.json()?;
        answer
            .get("result")
            .cloned()
            .ok_or_else(|| format!("{method} failed: {answer}").into())
    }

    /// Stops the adapter as an operator would, which removes its token file.
    fn stop(mut self) -> TestResult {
        let pid = self.child.id().to_string();
        std::process::Command::new("kill")
            .args(["-TERM", &pid])
            .status()?;
        assert!(self.child.wait()?.success());
        assert!(!std::path::Path::new(&self.token_file).exists());
        Ok(())
    }
}

impl Drop for Adapter {
    fn drop(&mut self) {
        // A test that fails early never stops the adapter itself.
        let _killed = self.child.kill();
        let _reaped = self.child.wait();
    }
}

fn message(id: &str, words: &str) -> Value {
    json!({"message": {"messageId": id, "role": "ROLE_USER", "parts": [{"text": words}]}})
}

/// A message continuing task `task`.
fn continuing(id: &str, words: &str, task: &str) -> Value {
    json!({"message": {
        "messageId": id, "role": "ROLE_USER", "taskId": task, "parts": [{"text": words}],
    }})
}

/// A message whose send answers at once, before the task settles.
fn immediate(id: &str, words: &str) -> Value {
    json!({
        "message": {"messageId": id, "role": "ROLE_USER", "parts": [{"text": words}]},
        "configuration": {"returnImmediately": true},
    })
}

fn error_code(answer: &Answer) -> TestValue<i64> {
    let document = answer.json()?;
    document
        .pointer("/error/code")
        .and_then(Value::as_i64)
        .ok_or_else(|| format!("no error: {document}").into())
}

/// Every route needs the token, and every call A2A-Version 1.0.
fn cards_and_access(adapter: &Adapter, (lead, worker): (&str, &str)) -> TestResult {
    let anonymous = request(
        (
            "GET",
            &format!("{}{lead}/.well-known/agent-card.json", adapter.url),
        ),
        &[],
        None,
    )?;
    assert_eq!(anonymous.status, 401);
    let card = adapter
        .get(&format!("{worker}/.well-known/agent-card.json"))?
        .json()?;
    assert_eq!(text(&card, "/name"), "worker");
    assert_eq!(
        text(&card, "/supportedInterfaces/0/protocolBinding"),
        "JSONRPC"
    );
    assert_eq!(
        text(&card, "/supportedInterfaces/0/url"),
        format!("{}{worker}", adapter.url)
    );
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": "GetTask", "params": {"id": "x"}});
    let bearer = format!("Bearer {}", adapter.token);
    let unversioned = request(
        ("POST", &format!("{}{lead}", adapter.url)),
        &[("Authorization", &bearer)],
        Some(body.to_string().as_bytes()),
    )?;
    assert_eq!(error_code(&unversioned)?, -32_009);
    Ok(())
}

/// A blocking send to A's own Agent answers once it completed; a resend replays it and
/// another message under the same ID conflicts. Input required, then answered by
/// continuing the task.
fn send_replay_and_continue(adapter: &Adapter, lead: &str) -> TestResult {
    let sending = (lead, "SendMessage");
    let sent = adapter.result(sending, &message("m-echo", "echo hello"))?;
    assert_eq!(
        text(&sent, "/task/status/state"),
        "TASK_STATE_COMPLETED",
        "{sent}"
    );
    assert_eq!(text(&sent, "/task/status/message/parts/0/text"), "hello");
    let again = adapter.result(sending, &message("m-echo", "echo hello"))?;
    assert_eq!(text(&again, "/task/id"), text(&sent, "/task/id"));
    let reused = adapter.call(sending, &message("m-echo", "echo other"))?;
    assert_eq!(error_code(&reused)?, -32_603, "{:?}", reused.json());
    let late = continuing("m-late", "echo late", &text(&sent, "/task/id"));
    let late = adapter.call(sending, &late)?;
    assert_eq!(error_code(&late)?, -32_004, "{:?}", late.json());
    let ended = adapter.call(
        (lead, "CancelTask"),
        &json!({"id": text(&sent, "/task/id")}),
    )?;
    assert_eq!(error_code(&ended)?, -32_002, "{:?}", ended.json());
    let unknown = continuing("m-unknown", "hello", &"00".repeat(16));
    let unknown = adapter.call(sending, &unknown)?;
    assert_eq!(error_code(&unknown)?, -32_001, "{:?}", unknown.json());
    // A malformed parameter is refused before delivery: no task is left behind, so the
    // message ID is still free for other content.
    let malformed = json!({
        "message": {"messageId": "m-bad", "role": "ROLE_USER", "parts": [{"text": "echo bad"}]},
        "configuration": {"historyLength": -1},
    });
    let malformed = adapter.call(sending, &malformed)?;
    assert_eq!(error_code(&malformed)?, -32_602, "{:?}", malformed.json());
    let fresh = adapter.result(sending, &message("m-bad", "echo other"))?;
    assert_eq!(text(&fresh, "/task/status/message/parts/0/text"), "other");

    let asked = adapter.result(sending, &message("m-ask", "ask colour?"))?;
    assert_eq!(
        text(&asked, "/task/status/state"),
        "TASK_STATE_INPUT_REQUIRED",
        "{asked}"
    );
    assert_eq!(text(&asked, "/task/status/message/parts/0/text"), "colour?");
    let reply = continuing("m-blue", "blue", &text(&asked, "/task/id"));
    let answered = adapter.result(sending, &reply)?;
    assert_eq!(
        text(&answered, "/task/status/message/parts/0/text"),
        "colour? blue",
        "{answered}"
    );
    let history = answered
        .pointer("/task/history")
        .and_then(Value::as_array)
        .map(Vec::len);
    assert!(history >= Some(3), "{answered}");

    // A streamed answer follows the task past its wait until it waits again or ends.
    let asked = adapter.result(sending, &message("m-ask-2", "ask shape?"))?;
    let reply = continuing("m-round", "round", &text(&asked, "/task/id"));
    let streamed = adapter
        .call((lead, "SendStreamingMessage"), &reply)?
        .events()?;
    let end = streamed.last().ok_or("empty stream")?;
    assert_eq!(
        text(end, "/result/statusUpdate/status/state"),
        "TASK_STATE_COMPLETED",
        "{streamed:?}"
    );
    Ok(())
}

/// A stopped local Agent is unavailable, not a conflict.
fn stopped_agent(adapter: &Adapter, lab: &AgentLab, lead: &str) -> TestResult {
    lab.a.run_ok(&["agent", "stop", "lead"])?;
    let refused = adapter.call((lead, "SendMessage"), &message("m-stopped", "echo x"))?;
    let document = refused.json()?;
    assert_eq!(
        document.pointer("/error/data/0/reason"),
        Some(&json!("AGENT_UNAVAILABLE")),
        "{document}"
    );
    Ok(())
}

/// B's Agent, streamed through A: the task, its progress, the artifact, the end. Get and
/// list agree with the stream; unknown tasks and push configs are refused.
fn stream_get_and_list(
    adapter: &Adapter,
    lab: &AgentLab,
    (lead, worker): (&str, &str),
) -> TestResult {
    let instruction = format!("fetch {} docs /input.txt fetched.bin", lab.a.id());
    let streamed = adapter.call(
        (worker, "SendStreamingMessage"),
        &message("m-fetch", &instruction),
    )?;
    assert_eq!(streamed.header("content-type"), Some("text/event-stream"));
    let events = streamed.events()?;
    let first = events.first().ok_or("empty stream")?;
    let fetched = text(first, "/result/task/id");
    assert_eq!(fetched.len(), 32, "{first}");
    let artifact = events
        .iter()
        .find_map(|event| event.pointer("/result/artifactUpdate/artifact"))
        .ok_or("no artifact update")?;
    let last = events.last().ok_or("empty stream")?;
    assert_eq!(
        text(last, "/result/statusUpdate/status/state"),
        "TASK_STATE_COMPLETED",
        "{events:?}"
    );
    let url = text(artifact, "/parts/0/url");
    let path = url
        .strip_prefix(&adapter.url)
        .ok_or("artifact URL is not the adapter's")?;
    assert_eq!(adapter.get(path)?.body, content(INPUT_LEN));

    let got = adapter.result((worker, "GetTask"), &json!({"id": fetched}))?;
    assert_eq!(text(&got, "/artifacts/0/name"), "fetched.bin");
    let listed = adapter.result(
        (worker, "ListTasks"),
        &json!({"status": "TASK_STATE_COMPLETED"}),
    )?;
    assert_eq!(text(&listed, "/tasks/0/id"), fetched, "{listed}");
    let with_artifacts = adapter.result(
        (worker, "ListTasks"),
        &json!({"status": "TASK_STATE_COMPLETED", "includeArtifacts": true}),
    )?;
    assert_eq!(
        text(&with_artifacts, "/tasks/0/artifacts/0/name"),
        "fetched.bin"
    );
    assert!(
        with_artifacts
            .pointer("/tasks/0/history")
            .and_then(serde_json::Value::as_array)
            .is_none_or(Vec::is_empty),
        "{with_artifacts}"
    );
    let unknown = adapter.call((worker, "GetTask"), &json!({"id": "00".repeat(16)}))?;
    assert_eq!(error_code(&unknown)?, -32_001);
    let push = adapter.call((lead, "CreateTaskPushNotificationConfig"), &json!({}))?;
    assert_eq!(error_code(&push)?, -32_003);
    let ended = adapter.call((worker, "SubscribeToTask"), &json!({"id": fetched}))?;
    assert_eq!(error_code(&ended)?, -32_004);
    Ok(())
}

/// A delegation by A's own Agent publishes an artifact the adapter serves from A.
fn local_artifact(adapter: &Adapter, lab: &AgentLab, lead: &str) -> TestResult {
    let delegate = format!(
        "delegate {} worker fetch {} docs /input.txt report.txt",
        lab.b.id(),
        lab.a.id()
    );
    let delegated = adapter.result((lead, "SendMessage"), &message("m-delegate", &delegate))?;
    let local_url = text(&delegated, "/task/artifacts/0/parts/0/url");
    let local_path = local_url
        .strip_prefix(&adapter.url)
        .ok_or("not the adapter's URL")?;
    let downloaded = adapter.get(local_path)?;
    assert_eq!(
        downloaded.header("content-length"),
        Some(INPUT_LEN.to_string().as_str())
    );
    assert_eq!(downloaded.body, content(INPUT_LEN));
    Ok(())
}

/// Subscriptions follow a running task to its end; cancel ends local and remote tasks.
fn subscribe_and_cancel(adapter: &Adapter, (lead, worker): (&str, &str)) -> TestResult {
    let running = adapter.result((worker, "SendMessage"), &immediate("m-quick", "sleep 1500"))?;
    let followed = adapter.call(
        (worker, "SubscribeToTask"),
        &json!({"id": text(&running, "/task/id")}),
    )?;
    let followed = followed.events()?;
    assert!(
        followed
            .first()
            .is_some_and(|event| event.pointer("/result/task").is_some())
    );
    let end = followed.last().ok_or("empty subscription")?;
    assert_eq!(
        text(end, "/result/statusUpdate/status/state"),
        "TASK_STATE_COMPLETED"
    );
    for (route, id) in [(lead, "m-long-lead"), (worker, "m-long-worker")] {
        let started = adapter.result((route, "SendMessage"), &immediate(id, "sleep 60000"))?;
        // A malformed cancel cancels nothing.
        let malformed = json!({"id": text(&started, "/task/id"), "historyLength": "x"});
        let malformed = adapter.call((route, "CancelTask"), &malformed)?;
        assert_eq!(error_code(&malformed)?, -32_602, "{:?}", malformed.json());
        let still = adapter.result(
            (route, "GetTask"),
            &json!({"id": text(&started, "/task/id")}),
        )?;
        assert_eq!(
            text(&still, "/status/state"),
            "TASK_STATE_WORKING",
            "{still}"
        );
        let cancelled = adapter.result(
            (route, "CancelTask"),
            &json!({"id": text(&started, "/task/id")}),
        )?;
        assert_eq!(
            text(&cancelled, "/status/state"),
            "TASK_STATE_CANCELED",
            "{cancelled}"
        );
    }
    Ok(())
}

#[test]
fn a2a_clients_use_local_and_remote_agents() -> TestResult {
    let lab = AgentLab::start("a2a")?;
    let adapter = Adapter::start(&lab)?;
    let worker = format!("/endpoints/{}/agents/worker", lab.b.id());
    let routes = ("/agents/lead", worker.as_str());
    cards_and_access(&adapter, routes)?;
    send_replay_and_continue(&adapter, routes.0)?;
    stream_get_and_list(&adapter, &lab, routes)?;
    local_artifact(&adapter, &lab, routes.0)?;
    subscribe_and_cancel(&adapter, routes)?;
    stopped_agent(&adapter, &lab, routes.0)?;
    adapter.stop()?;
    lab.shutdown()
}
