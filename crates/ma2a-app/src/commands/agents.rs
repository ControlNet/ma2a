//! `ma2a agent`: the Agents this Runtime hosts, through the local API as their operator,
//! or another Endpoint's with `--endpoint`, through the stream plane as this Endpoint.

use std::{path::Path, time::Duration};

use ma2a_core::service::{AgentMessage, MessagePart};
use ma2a_runtime::ipc::IpcPaths;
use serde_json::{Map, Value, json};

use crate::{
    AppError,
    cli::agents::{AgentArgs, AgentCommand, DefineArgs, SubmitArgs, TaskTarget},
    commands::{
        remote::agent::{self as remote, message},
        workflows::{command, request_fields},
    },
    output,
};

/// Pause between two reads of a local task that has not ended yet.
const POLL: Duration = Duration::from_millis(250);

pub(crate) async fn run(
    state_dir: &Path,
    paths: IpcPaths,
    arguments: AgentArgs,
) -> Result<(), AppError> {
    match arguments.endpoint {
        Some(endpoint) => remote::run((&paths, &endpoint), arguments.command).await,
        None => local((state_dir, paths), arguments.command).await,
    }
}

type Runtime<'a> = (&'a Path, IpcPaths);

async fn local(runtime: Runtime<'_>, agent_command: AgentCommand) -> Result<(), AppError> {
    let (operation, fields) = match agent_command {
        AgentCommand::Define(arguments) => ("agent_define", define(arguments)?),
        AgentCommand::Undefine { agent } => ("agent_undefine", named(&agent)?),
        AgentCommand::Start { agent } => ("agent_start", named(&agent)?),
        AgentCommand::Stop { agent } => ("agent_stop", named(&agent)?),
        AgentCommand::List { after } => ("agent_list", object([("after", json!(after))])),
        AgentCommand::Tasks { agent, after } => (
            "agent_task_list",
            object([("agent", json!(agent)), ("after", json!(after))]),
        ),
        AgentCommand::Task(target) => (
            "agent_task_inspect",
            object([("task_id", json!(target.task))]),
        ),
        AgentCommand::Watch { target, after } => return watch(&runtime, &target.task, after).await,
        AgentCommand::Submit(arguments) => return submit(&runtime, arguments).await,
        AgentCommand::Send {
            target,
            message: parts,
            request_id,
        } => {
            let mut fields = mutation(request_id)?;
            fields.insert("task_id".to_owned(), json!(target.task));
            fields.insert("message".to_owned(), message_json(&message(&parts)?));
            ("agent_task_send", fields)
        }
        AgentCommand::Cancel(TaskTarget { task, .. }) => {
            let mut fields = request_fields()?;
            fields.insert("task_id".to_owned(), json!(task));
            ("agent_task_cancel", fields)
        }
        AgentCommand::Discover { .. } | AgentCommand::Artifact { .. } => {
            return Err(AppError::Usage(
                "discover and artifact act on another Endpoint; add --endpoint",
            ));
        }
    };
    print(&payload(&runtime, operation, fields).await?)
}

fn object<const N: usize>(fields: [(&str, Value); N]) -> Map<String, Value> {
    fields
        .into_iter()
        .map(|(name, value)| (name.to_owned(), value))
        .collect()
}

fn mutation(request_id: Option<String>) -> Result<Map<String, Value>, AppError> {
    let mut fields = request_fields()?;
    if let Some(request_id) = request_id {
        fields.insert("request_id".to_owned(), json!(request_id));
    }
    Ok(fields)
}

fn named(agent: &str) -> Result<Map<String, Value>, AppError> {
    let mut fields = request_fields()?;
    fields.insert("agent".to_owned(), json!(agent));
    Ok(fields)
}

fn define(arguments: DefineArgs) -> Result<Map<String, Value>, AppError> {
    let (program, args) = arguments
        .argv
        .split_first()
        .ok_or(AppError::Usage("define requires a PROGRAM after --"))?;
    let env = arguments
        .env
        .iter()
        .map(|entry| {
            let (name, value) = entry
                .split_once('=')
                .ok_or(AppError::Usage("--env takes NAME=VALUE"))?;
            Ok(json!({"name": name, "value": value}))
        })
        .collect::<Result<Vec<_>, AppError>>()?;
    let mut fields = named(&arguments.agent)?;
    fields.extend(object([
        ("description", json!(arguments.description)),
        ("program", json!(program)),
        ("args", json!(args)),
        ("cwd", json!(arguments.cwd)),
        ("env", json!(env)),
        ("tools", json!(arguments.tools)),
        ("max_tasks", json!(arguments.max_tasks)),
    ]));
    Ok(fields)
}

/// The local API form of one message.
fn message_json(message: &AgentMessage) -> Value {
    let parts: Vec<Value> = message
        .parts()
        .iter()
        .map(|part| match part {
            MessagePart::Text(text) => json!({"kind": "text", "content": text}),
            MessagePart::Data(data) => json!({"kind": "data", "content": data}),
        })
        .collect();
    json!({"role": message.role().name(), "parts": parts})
}

async fn submit(runtime: &Runtime<'_>, arguments: SubmitArgs) -> Result<(), AppError> {
    let mut fields = mutation(arguments.request_id)?;
    fields.insert("agent".to_owned(), json!(arguments.agent));
    fields.insert(
        "message".to_owned(),
        message_json(&message(&arguments.message)?),
    );
    let submitted = payload(runtime, "agent_task_submit", fields).await?;
    print(&submitted)?;
    if !arguments.wait {
        return Ok(());
    }
    let task = submitted
        .pointer("/task/task_id")
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::Protocol("the submit answer has no task".to_owned()))?
        .to_owned();
    ended(runtime, &task, None).await?;
    print(&inspect(runtime, &task).await?)
}

/// Prints every event after `after`, then the task once it ended.
async fn watch(runtime: &Runtime<'_>, task: &str, after: u32) -> Result<(), AppError> {
    let ended = ended(runtime, task, Some(after)).await?;
    print(&ended)
}

/// Waits until a task ended, printing its events when `after` is given; answers its view.
async fn ended(
    runtime: &Runtime<'_>,
    task: &str,
    mut after: Option<u32>,
) -> Result<Value, AppError> {
    loop {
        // The state is read before the events: once it is terminal, every event is there.
        let details = inspect(runtime, task).await?;
        let terminal = matches!(
            details.pointer("/task/state").and_then(Value::as_str),
            Some("completed" | "failed" | "cancelled")
        );
        let mut more = true;
        while let (Some(seen), true) = (after, more) {
            let fields = object([("task_id", json!(task)), ("after", json!(seen))]);
            let page = payload(runtime, "agent_task_events", fields).await?;
            for event in page
                .get("events")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                print(event)?;
                after = event
                    .get("seq")
                    .and_then(Value::as_u64)
                    .and_then(|seq| u32::try_from(seq).ok());
            }
            more = page.get("more").and_then(Value::as_bool).unwrap_or(false);
        }
        if terminal {
            return Ok(details.get("task").cloned().unwrap_or(Value::Null));
        }
        tokio::time::sleep(POLL).await;
    }
}

async fn inspect(runtime: &Runtime<'_>, task: &str) -> Result<Value, AppError> {
    let fields = object([("task_id", json!(task))]);
    payload(runtime, "agent_task_inspect", fields).await
}

/// Runs one local API command and answers its result payload.
async fn payload(
    (state_dir, paths): &Runtime<'_>,
    operation: &str,
    fields: Map<String, Value>,
) -> Result<Value, AppError> {
    let response = crate::request((state_dir, paths.clone()), command(operation, fields)?).await?;
    let document: Value = serde_json::from_slice(&response).map_err(std::io::Error::other)?;
    output::reject_runtime_error(&document)?;
    Ok(document
        .pointer("/result/payload")
        .cloned()
        .unwrap_or(Value::Null))
}

fn print(value: &Value) -> Result<(), AppError> {
    use std::io::Write as _;
    writeln!(std::io::stdout().lock(), "{value}")?;
    Ok(())
}
