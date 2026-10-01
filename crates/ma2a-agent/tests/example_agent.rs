//! The reference Agent as a real process: this test plays the Runtime on its stdio and
//! answers every tool call from a script.

use std::{process::Stdio, time::Duration};

use ma2a_agent::protocol::{
    AgentToRuntime, Message, Origin, ReportedState, RuntimeToAgent, decode_line, encode_line,
};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
};

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;
type TestValue<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

struct Agent {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    seq: u64,
}

impl Agent {
    async fn spawn() -> TestValue<Self> {
        let mut child = Command::new(env!("CARGO_BIN_EXE_ma2a-example-agent"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = BufReader::new(child.stdout.take().ok_or("no stdout")?);
        let mut agent = Self {
            child,
            stdin,
            stdout,
            seq: 0,
        };
        assert_eq!(agent.next().await?, AgentToRuntime::Ready { protocol: 1 });
        Ok(agent)
    }

    async fn deliver(&mut self, delivery: impl FnOnce(u64) -> RuntimeToAgent) -> TestResult {
        self.seq += 1;
        self.stdin
            .write_all(&encode_line(&delivery(self.seq))?)
            .await?;
        Ok(())
    }

    async fn submit(&mut self, text: &str) -> TestResult {
        self.deliver(|seq| RuntimeToAgent::Task {
            seq,
            task: "t".to_owned(),
            from: Origin {
                endpoint: "creator".to_owned(),
                agent: None,
            },
            message: Message::user_text(text),
        })
        .await
    }

    async fn next(&mut self) -> TestValue<AgentToRuntime> {
        let mut line = String::new();
        let read = self.stdout.read_line(&mut line);
        if tokio::time::timeout(Duration::from_secs(10), read).await?? == 0 {
            return Err("the Agent closed stdout".into());
        }
        Ok(decode_line(line.trim_end().as_bytes())?)
    }

    /// Answers tool calls from `script` in order until the task ends; returns the final
    /// state, its text, and every progress text.
    async fn run(
        &mut self,
        script: &[(&str, Value)],
    ) -> TestValue<(ReportedState, String, Vec<String>)> {
        let mut script = script.iter();
        let mut progress = Vec::new();
        loop {
            match self.next().await? {
                AgentToRuntime::Tool { call, name, .. } => {
                    let (expected, result) = script.next().ok_or("an unscripted tool call")?;
                    assert_eq!(&name, expected);
                    let result = result.clone();
                    self.deliver(|seq| RuntimeToAgent::ToolResult {
                        seq,
                        task: "t".to_owned(),
                        call,
                        ok: Some(result),
                        error: None,
                    })
                    .await?;
                }
                AgentToRuntime::Progress { text, .. } => progress.push(text),
                AgentToRuntime::State {
                    state:
                        state @ (ReportedState::Completed
                        | ReportedState::Failed
                        | ReportedState::Cancelled),
                    message,
                    error,
                    ..
                } => {
                    let text = message.map(|message| message.text()).or(error);
                    assert!(script.next().is_none(), "a scripted call was never made");
                    return Ok((state, text.unwrap_or_default(), progress));
                }
                _ => {}
            }
        }
    }
}

#[tokio::test]
async fn simple_commands_complete_fail_and_report_progress() -> TestResult {
    let cases = [
        (
            "echo hello  world",
            ReportedState::Completed,
            "hello  world",
        ),
        ("progress 3", ReportedState::Completed, "done 3"),
        ("fail on purpose", ReportedState::Failed, "on purpose"),
        ("dance", ReportedState::Failed, "unknown command `dance`"),
    ];
    for (input, state, text) in cases {
        let mut agent = Agent::spawn().await?;
        agent.submit(input).await?;
        let (ended, result, progress) = agent.run(&[]).await?;
        assert_eq!((ended, result.as_str()), (state, text), "{input}");
        if input.starts_with("progress") {
            assert_eq!(progress, ["step 1 of 3", "step 2 of 3", "step 3 of 3"]);
        }
    }
    Ok(())
}

#[tokio::test]
async fn ask_waits_for_the_creator_and_crash_ends_the_process() -> TestResult {
    let mut agent = Agent::spawn().await?;
    agent.submit("ask colour?").await?;
    loop {
        if let AgentToRuntime::State {
            state: ReportedState::Waiting,
            message,
            ..
        } = agent.next().await?
        {
            assert_eq!(
                message.map(|message| message.text()).as_deref(),
                Some("colour?")
            );
            break;
        }
    }
    agent
        .deliver(|seq| RuntimeToAgent::Message {
            seq,
            task: "t".to_owned(),
            message: Message::user_text("blue"),
        })
        .await?;
    let (state, text, _) = agent.run(&[]).await?;
    assert_eq!(
        (state, text.as_str()),
        (ReportedState::Completed, "colour? blue")
    );

    let mut agent = Agent::spawn().await?;
    agent.submit("crash").await?;
    let status = tokio::time::timeout(Duration::from_secs(10), agent.child.wait()).await??;
    assert_eq!(status.code(), Some(70));
    Ok(())
}

#[tokio::test]
async fn fetch_and_delegate_drive_their_tools_in_order() -> TestResult {
    let mut agent = Agent::spawn().await?;
    agent.submit("fetch peer docs /a.txt a.txt").await?;
    let transferred = json!({"size": 3, "blake3": "00"});
    let published = json!({"name": "a.txt", "media_type": "text/plain", "size": 3, "blake3": "00"});
    let script = [
        ("transfer.get", transferred.clone()),
        ("artifact.publish", published.clone()),
    ];
    let (state, text, progress) = agent.run(&script).await?;
    assert_eq!(
        (state, text.as_str()),
        (ReportedState::Completed, "fetched a.txt (3 bytes)")
    );
    assert_eq!(progress, ["fetching docs:/a.txt", "fetched 3 bytes"]);

    let mut agent = Agent::spawn().await?;
    agent
        .submit("delegate peer helper fetch home docs /a.txt a.txt")
        .await?;
    let running = json!({"task": "ab", "state": "running", "artifacts": []});
    let done = json!({
        "task": "ab", "state": "completed", "progress": "fetched 3 bytes",
        "result": {"role": "agent", "parts": [{"kind": "text", "text": "fetched a.txt"}]},
        "artifacts": [{"name": "a.txt", "media_type": "text/plain", "size": 3, "blake3": "00"}],
    });
    let script = [
        ("agent.submit", running),
        ("agent.wait", done),
        ("agent.artifact", transferred),
        ("artifact.publish", published),
    ];
    let (state, text, _) = agent.run(&script).await?;
    assert_eq!(
        (state, text.as_str()),
        (ReportedState::Completed, "helper completed: fetched a.txt")
    );
    Ok(())
}
