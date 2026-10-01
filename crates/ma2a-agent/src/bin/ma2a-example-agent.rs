//! `ma2a-example-agent`: a deterministic reference Agent and an example of the SDK.
//!
//! It is not a language model. The first word of each task's text is a command, and the
//! rest are its arguments:
//!
//! | Command | Effect |
//! | --- | --- |
//! | `echo TEXT` | completes with `TEXT` |
//! | `ask QUESTION` | waits for the creator's answer, then completes with it |
//! | `sleep MS` | reports progress every 100 ms for `MS` milliseconds, then completes |
//! | `progress N` | reports `N` progress events, then completes |
//! | `fail TEXT` | fails with `TEXT` |
//! | `crash` | exits the whole Agent process at once |
//! | `fetch ENDPOINT ROOT PATH NAME` | downloads a remote file and publishes it as artifact `NAME` |
//! | `exec ENDPOINT PROGRAM ARGS…` | runs a remote program and completes with its stdout |
//! | `delegate ENDPOINT AGENT TEXT…` | submits `TEXT` to a remote Agent, waits, republishes its artifacts, and completes with its result |
//! | `tool NAME JSON` | calls one tool with raw arguments and completes with its result |

use std::{process::ExitCode, time::Duration};

use ma2a_agent::{
    protocol::{Message, Part, ToolError},
    sdk::{self, Task},
    tools::{
        AgentArtifactArgs, AgentSubmitArgs, AgentWait, ArtifactPublish, ExecOutput, ExecRun,
        TaskView, Tool, TransferGet, Transferred,
    },
};
use serde_json::Value;

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    match sdk::serve(run).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("ma2a-example-agent: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run(mut task: Task) {
    let text = task.input().text();
    let (command, rest) = text
        .trim()
        .split_once(char::is_whitespace)
        .unwrap_or_else(|| (text.trim(), ""));
    let rest = rest.trim().to_owned();
    let words: Vec<String> = rest.split_whitespace().map(str::to_owned).collect();
    if command == "tool" {
        match raw_tool(&task, &rest).await {
            Ok(result) => task.complete(Some(result)),
            Err(error) => task.fail(&error),
        }
        return;
    }
    let outcome = match command {
        "echo" => Ok(rest),
        "ask" => match task.ask(Message::agent_text(rest.clone())).await {
            Some(answer) => Ok(format!("{rest} {}", answer.text())),
            None => return,
        },
        "sleep" => match sleep(&task, &rest).await {
            Some(outcome) => outcome,
            None => return,
        },
        "progress" => progress(&task, &rest),
        "fail" => Err(rest),
        "crash" => std::process::exit(70),
        "fetch" => fetch(&task, &words).await,
        "exec" => exec(&task, &words).await,
        "delegate" => delegate(&task, &words).await,
        _ => Err(format!("unknown command `{command}`")),
    };
    match outcome {
        Ok(result) => task.complete(Some(Message::agent_text(result))),
        Err(error) => task.fail(&error),
    }
}

type Outcome = Result<String, String>;

fn failure(error: &ToolError) -> String {
    format!("{}: {}", error.code, error.message)
}

/// Sleeps in 100 ms steps; `None` means the task was cancelled.
async fn sleep(task: &Task, rest: &str) -> Option<Outcome> {
    let Ok(total) = rest.parse::<u64>() else {
        return Some(Err("usage: sleep MS".to_owned()));
    };
    let mut slept = 0;
    while slept < total {
        let step = (total - slept).min(100);
        tokio::select! {
            () = tokio::time::sleep(Duration::from_millis(step)) => {}
            () = task.cancelled() => return None,
        }
        slept += step;
        task.progress(&format!("slept {slept} of {total} ms"));
    }
    Some(Ok(format!("slept {total} ms")))
}

fn progress(task: &Task, rest: &str) -> Outcome {
    let steps = rest.parse::<u32>().map_err(|_| "usage: progress N")?;
    for step in 1..=steps {
        task.progress(&format!("step {step} of {steps}"));
    }
    Ok(format!("done {steps}"))
}

async fn fetch(task: &Task, words: &[String]) -> Outcome {
    let [endpoint, root, path, name] = words else {
        return Err("usage: fetch ENDPOINT ROOT PATH NAME".to_owned());
    };
    task.progress(&format!("fetching {root}:{path}"));
    let get = TransferGet {
        endpoint: endpoint.clone(),
        root: root.clone(),
        path: path.clone(),
        to: name.clone(),
    };
    let fetched: Transferred = task
        .call_as(Tool::TransferGet, &get)
        .await
        .map_err(|error| failure(&error))?;
    task.progress(&format!("fetched {} bytes", fetched.size));
    publish(task, name).await?;
    Ok(format!("fetched {name} ({} bytes)", fetched.size))
}

async fn publish(task: &Task, name: &str) -> Result<(), String> {
    let publish = ArtifactPublish {
        name: name.to_owned(),
        path: name.to_owned(),
        media_type: None,
    };
    task.call(Tool::ArtifactPublish, &publish)
        .await
        .map(|_| ())
        .map_err(|error| failure(&error))
}

async fn exec(task: &Task, words: &[String]) -> Outcome {
    let [endpoint, program, args @ ..] = words else {
        return Err("usage: exec ENDPOINT PROGRAM ARGS…".to_owned());
    };
    let run = ExecRun {
        endpoint: endpoint.clone(),
        program: program.clone(),
        args: args.to_vec(),
        cwd: None,
        env: std::collections::BTreeMap::new(),
        timeout_ms: None,
    };
    let output: ExecOutput = task
        .call_as(Tool::ExecRun, &run)
        .await
        .map_err(|error| failure(&error))?;
    match output.exit_code {
        Some(0) => Ok(output.stdout.trim_end().to_owned()),
        _ => Err(format!("{} exited: {}", program, output.reason)),
    }
}

async fn delegate(task: &Task, words: &[String]) -> Outcome {
    let [endpoint, agent, text @ ..] = words else {
        return Err("usage: delegate ENDPOINT AGENT TEXT…".to_owned());
    };
    let submit = AgentSubmitArgs {
        endpoint: endpoint.clone(),
        agent: agent.clone(),
        message: Message::user_text(text.join(" ")),
    };
    let submitted: TaskView = task
        .call_as(Tool::AgentSubmit, &submit)
        .await
        .map_err(|error| failure(&error))?;
    task.progress(&format!("delegated to {agent} as {}", submitted.task));
    let mut view = submitted;
    while !matches!(view.state.as_str(), "completed" | "failed" | "cancelled") {
        let wait = AgentWait {
            endpoint: endpoint.clone(),
            agent: agent.clone(),
            task: view.task.clone(),
            timeout_ms: None,
        };
        view = task
            .call_as(Tool::AgentWait, &wait)
            .await
            .map_err(|error| failure(&error))?;
        if let Some(progress) = &view.progress {
            task.progress(&format!("{agent}: {progress}"));
        }
    }
    if view.state != "completed" {
        let why = view.error.or(view.reason).unwrap_or_default();
        return Err(format!("{agent} {}: {why}", view.state));
    }
    for artifact in &view.artifacts {
        let download = AgentArtifactArgs {
            endpoint: endpoint.clone(),
            agent: agent.clone(),
            task: view.task.clone(),
            name: artifact.name.clone(),
            to: artifact.name.clone(),
        };
        task.call(Tool::AgentArtifact, &download)
            .await
            .map_err(|error| failure(&error))?;
        publish(task, &artifact.name).await?;
    }
    let result = view.result.map(|result| result.text()).unwrap_or_default();
    Ok(format!("{agent} completed: {result}"))
}

/// Calls one tool by name with raw JSON arguments and completes with its result as data.
async fn raw_tool(task: &Task, rest: &str) -> Result<Message, String> {
    let (name, args) = rest.split_once(char::is_whitespace).unwrap_or((rest, "{}"));
    let Some(tool) = Tool::from_name(name) else {
        return Err(format!("unknown tool `{name}`"));
    };
    let args: Value = serde_json::from_str(args).map_err(|error| error.to_string())?;
    let result = task
        .call(tool, &args)
        .await
        .map_err(|error| failure(&error))?;
    Ok(Message {
        role: ma2a_agent::protocol::Role::Agent,
        parts: vec![Part::Data { data: result }],
    })
}
