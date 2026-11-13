//! The SDK loop over in-memory pipes: readiness, deduplication, acknowledgements, tool
//! calls, input requests, and cancellation.

use std::time::Duration;

use ma2a_agent::{
    protocol::{
        AgentToRuntime, Message, Origin, ReportedState, RuntimeToAgent, decode_line, encode_line,
    },
    sdk::{self, Task},
    tools::Tool,
};
use serde_json::json;
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader, DuplexStream};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// The Runtime's ends of one served Agent.
struct Runtime {
    to_agent: DuplexStream,
    from_agent: BufReader<DuplexStream>,
}

impl Runtime {
    fn start() -> Self {
        let (to_agent, agent_input) = tokio::io::duplex(1 << 16);
        let (agent_output, from_agent) = tokio::io::duplex(1 << 16);
        tokio::spawn(sdk::serve_on((agent_input, agent_output), handle));
        Self {
            to_agent,
            from_agent: BufReader::new(from_agent),
        }
    }

    async fn raw(&mut self, bytes: &[u8]) -> TestResult {
        self.to_agent.write_all(bytes).await?;
        Ok(())
    }

    async fn send(&mut self, delivery: &RuntimeToAgent) -> TestResult {
        self.raw(&encode_line(delivery)?).await
    }

    async fn next(&mut self) -> Result<AgentToRuntime, Box<dyn std::error::Error>> {
        let mut line = String::new();
        tokio::time::timeout(Duration::from_secs(5), self.from_agent.read_line(&mut line))
            .await??;
        Ok(decode_line(line.trim_end().as_bytes())?)
    }

    /// Reads lines until `want` matches one, returning every line read before it.
    async fn until(
        &mut self,
        mut want: impl FnMut(&AgentToRuntime) -> bool,
    ) -> Result<Vec<AgentToRuntime>, Box<dyn std::error::Error>> {
        let mut seen = Vec::new();
        loop {
            let line = self.next().await?;
            if want(&line) {
                return Ok(seen);
            }
            seen.push(line);
        }
    }
}

fn task(seq: u64, id: &str, text: &str) -> RuntimeToAgent {
    RuntimeToAgent::Task {
        seq,
        task: id.to_owned(),
        from: Origin {
            endpoint: "creator".to_owned(),
            agent: None,
        },
        message: Message::user_text(text),
    }
}

fn ended(id: &str, state: ReportedState) -> impl Fn(&AgentToRuntime) -> bool {
    move |line| {
        matches!(line, AgentToRuntime::State { task, state: reported, .. }
            if task == id && *reported == state)
    }
}

async fn handle(mut task: Task) {
    let text = task.input().text();
    match text.as_str() {
        "echo" => task.complete(Some(Message::agent_text("echoed"))),
        "ask" => {
            if let Some(answer) = task.ask(Message::agent_text("which?")).await {
                task.complete(Some(Message::agent_text(answer.text())));
            }
        }
        "tool" => match task.call(Tool::FilesStat, &json!({"endpoint": "e"})).await {
            Ok(value) => task.complete(Some(Message::agent_text(value.to_string()))),
            Err(error) => task.fail(&error.code),
        },
        "huge-tool" => {
            let args = json!({"text": "x".repeat(ma2a_agent::protocol::MAX_TOOL_ARGS_LEN)});
            match task.call(Tool::FilesStat, &args).await {
                Ok(_) => task.complete(None),
                Err(error) => task.fail(&error.code),
            }
        }
        "cancel-me" => task.cancelled().await,
        "huge-result" => {
            let text = "x".repeat(ma2a_agent::protocol::MAX_MESSAGE_BYTES + 1);
            task.complete(Some(Message::agent_text(text)));
        }
        "nul-result" => task.complete(Some(Message::agent_text("a\0b"))),
        "huge-question" => {
            let text = "x".repeat(ma2a_agent::protocol::MAX_MESSAGE_BYTES + 1);
            let answer = task.ask(Message::agent_text(text)).await;
            assert!(answer.is_none());
            task.complete(Some(Message::agent_text("too late")));
        }
        _ => {}
    }
}

#[tokio::test]
async fn the_agent_announces_readiness_and_completes_a_task_once() -> TestResult {
    let mut runtime = Runtime::start();
    assert_eq!(runtime.next().await?, AgentToRuntime::Ready { protocol: 1 });
    // A cut-short line, an empty line, and garbage are skipped.
    runtime.raw(b"{\"t\":\"task\",\"se\n\nnonsense\n").await?;
    runtime.send(&task(1, "a", "echo")).await?;
    let before = runtime.until(ended("a", ReportedState::Completed)).await?;
    let mut acks = before
        .iter()
        .filter(|line| **line == AgentToRuntime::Ack { seq: 1 })
        .count();
    // A re-sent delivery is acknowledged again but never processed twice.
    runtime.send(&task(1, "a", "echo")).await?;
    while acks < 2 {
        match runtime.next().await? {
            AgentToRuntime::Ack { seq: 1 } => acks += 1,
            AgentToRuntime::State { .. } => return Err("a duplicate delivery ran".into()),
            _ => {}
        }
    }
    Ok(())
}

#[tokio::test]
async fn tool_calls_resolve_by_their_call_identifier() -> TestResult {
    let mut runtime = Runtime::start();
    runtime.next().await?;
    runtime.send(&task(1, "a", "tool")).await?;
    let mut call = None;
    runtime
        .until(|line| {
            if let AgentToRuntime::Tool {
                task,
                call: id,
                name,
                ..
            } = line
            {
                assert_eq!((task.as_str(), name.as_str()), ("a", "files.stat"));
                call = Some(id.clone());
                true
            } else {
                false
            }
        })
        .await?;
    let call = call.ok_or("no tool call")?;
    // A result for an unknown call is ignored; the matching one resolves the future.
    for (seq, id) in [(2, "nope"), (3, call.as_str())] {
        let result = RuntimeToAgent::ToolResult {
            seq,
            task: "a".to_owned(),
            call: id.to_owned(),
            ok: Some(json!({"size": seq})),
            error: None,
        };
        runtime.send(&result).await?;
    }
    let mut result = None;
    runtime
        .until(|line| {
            if let AgentToRuntime::State {
                state: ReportedState::Completed,
                message,
                ..
            } = line
            {
                result = message.as_ref().map(Message::text);
                true
            } else {
                false
            }
        })
        .await?;
    assert_eq!(result.as_deref(), Some(r#"{"size":3}"#));
    Ok(())
}

#[tokio::test]
async fn a_waiting_task_resumes_with_the_creators_next_message() -> TestResult {
    let mut runtime = Runtime::start();
    runtime.next().await?;
    runtime.send(&task(1, "a", "ask")).await?;
    runtime.until(ended("a", ReportedState::Waiting)).await?;
    let answer = RuntimeToAgent::Message {
        seq: 2,
        task: "a".to_owned(),
        message: Message::user_text("blue"),
    };
    runtime.send(&answer).await?;
    runtime.until(ended("a", ReportedState::Running)).await?;
    runtime.until(ended("a", ReportedState::Completed)).await?;
    Ok(())
}

#[tokio::test]
async fn cancellation_and_a_dropped_handle_both_end_the_task() -> TestResult {
    let mut runtime = Runtime::start();
    runtime.next().await?;
    runtime.send(&task(1, "a", "cancel-me")).await?;
    runtime.send(&task(2, "b", "forgotten")).await?;
    runtime.until(ended("b", ReportedState::Failed)).await?;
    let cancel = RuntimeToAgent::Cancel {
        seq: 3,
        task: "a".to_owned(),
    };
    runtime.send(&cancel).await?;
    runtime.until(ended("a", ReportedState::Cancelled)).await?;
    Ok(())
}

#[tokio::test]
async fn a_tool_call_over_the_argument_bound_fails_at_once() -> TestResult {
    let mut runtime = Runtime::start();
    runtime.next().await?;
    runtime.send(&task(1, "a", "huge-tool")).await?;
    let before = runtime.until(ended("a", ReportedState::Failed)).await?;
    assert!(
        before
            .iter()
            .all(|line| !matches!(line, AgentToRuntime::Tool { .. })),
        "{before:?}"
    );
    Ok(())
}

/// The Runtime ignores a whole state report whose message is out of bounds, so the SDK must
/// never send one: a task it released would then stay running forever.
#[tokio::test]
async fn an_out_of_bounds_result_or_question_fails_the_task_instead_of_vanishing() -> TestResult {
    let mut runtime = Runtime::start();
    runtime.next().await?;
    for (seq, id, text) in [
        (1, "a", "huge-result"),
        (2, "b", "nul-result"),
        (3, "c", "huge-question"),
    ] {
        runtime.send(&task(seq, id, text)).await?;
        let mut reported = None;
        runtime
            .until(|line| match line {
                AgentToRuntime::State {
                    task,
                    state,
                    message,
                    error,
                } if task == id => {
                    reported = Some((*state, message.is_some(), error.is_some()));
                    true
                }
                _ => false,
            })
            .await?;
        assert_eq!(reported, Some((ReportedState::Failed, false, true)), "{id}");
    }
    // The question's task ended there: its later completion sends nothing more.
    runtime.send(&task(4, "d", "echo")).await?;
    let before = runtime.until(ended("d", ReportedState::Completed)).await?;
    assert!(
        !before
            .iter()
            .any(|line| matches!(line, AgentToRuntime::State { task, .. } if task == "c")),
        "{before:?}"
    );
    Ok(())
}
