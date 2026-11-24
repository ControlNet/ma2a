//! `ma2a agent --endpoint`: another Endpoint's Agents, used as this Endpoint.

use std::{fmt::Write as _, io::Write as _, path::Path, time::Duration};

use ma2a_core::service::{
    AgentAccept, AgentCard, AgentId, AgentMessage, AgentReply, AgentRequest, AgentSubmit,
    AgentTaskId, MessagePart, MessageRole, OpenBody, ServiceError,
};
use ma2a_runtime::{
    api::{agent_event_value, agent_task_details_value, agent_task_value},
    ipc::IpcPaths,
};
use ma2a_services::client::{self, ClientError};
use serde_json::{Value, json};

use super::{open, open_as};
use crate::{
    AppError,
    cli::agents::{AgentCommand, MessageArgs, SubmitArgs, TaskTarget},
};

/// One remote target: the local stream plane and the hosting Endpoint.
pub(crate) type Target<'a> = (&'a IpcPaths, &'a str);

/// Pause before reaching a target again that is restarting or unreachable.
const RETRY: Duration = Duration::from_secs(1);

/// Builds one `user` message from text parts, then data parts.
pub(crate) fn message(arguments: &MessageArgs) -> Result<AgentMessage, AppError> {
    let mut parts: Vec<MessagePart> = arguments
        .texts
        .iter()
        .cloned()
        .map(MessagePart::Text)
        .collect();
    for data in &arguments.data {
        serde_json::from_str::<Value>(data)
            .map_err(|_| AppError::Usage("--data takes one JSON value"))?;
        parts.push(MessagePart::Data(data.clone()));
    }
    AgentMessage::new(MessageRole::User, parts)
        .map_err(|_| AppError::Usage("a message needs 1-16 parts within 12 KiB"))
}

fn agent_id(name: &str) -> Result<AgentId, AppError> {
    AgentId::new(name).map_err(|_| AppError::Invalid(format!("invalid Agent name: {name}")))
}

fn task_id(text: &str) -> Result<AgentTaskId, AppError> {
    AgentTaskId::parse(text)
        .map_err(|_| AppError::Invalid("TASK_ID must be 32 lowercase hex characters".to_owned()))
}

/// The Agent and task a remote command names.
fn named(target: &TaskTarget) -> Result<(AgentId, AgentTaskId), AppError> {
    let agent = target
        .agent
        .as_deref()
        .ok_or(AppError::Usage("a remote task also needs --agent"))?;
    Ok((agent_id(agent)?, task_id(&target.task)?))
}

fn card(card: &AgentCard) -> Value {
    json!({
        "agent": card.id.as_str(),
        "description": card.description,
        "available": card.available,
        "max_tasks": card.max_tasks,
    })
}

fn view(reply: &AgentReply) -> Value {
    match reply {
        AgentReply::Cards(cards, next) => json!({
            "agents": cards.iter().map(card).collect::<Vec<_>>(),
            "next": next.as_ref().map(AgentId::as_str),
        }),
        AgentReply::Summary(summary) => agent_task_value(summary),
        AgentReply::Details(details) => agent_task_details_value(details),
        AgentReply::Page(tasks, next) => json!({
            "tasks": tasks.iter().map(agent_task_value).collect::<Vec<_>>(),
            "next": next.map(|id| id.to_string()),
        }),
    }
}

pub(crate) async fn run(target: Target<'_>, command: AgentCommand) -> Result<(), AppError> {
    let request = match command {
        AgentCommand::Discover { after } => {
            AgentRequest::Agents(after.as_deref().map(agent_id).transpose()?)
        }
        AgentCommand::Submit(arguments) => return submit(target, arguments).await,
        AgentCommand::Tasks { agent, after } => {
            let agent = agent.ok_or(AppError::Usage("remote tasks need the AGENT"))?;
            AgentRequest::Tasks(
                agent_id(&agent)?,
                after.as_deref().map(task_id).transpose()?,
            )
        }
        AgentCommand::Task(task) => {
            let (agent, task) = named(&task)?;
            AgentRequest::Get(agent, task)
        }
        AgentCommand::Watch {
            target: task,
            after,
        } => {
            let summary = follow(target, named(&task)?, Some(after)).await?;
            return print(&view(&summary));
        }
        AgentCommand::Send {
            target: task,
            message: parts,
            request_id,
        } => {
            let (agent, task) = named(&task)?;
            let body = OpenBody::Agent(AgentRequest::Send(agent, task, message(&parts)?));
            return print(&view(&call(target, body, request_id.as_deref()).await?.1));
        }
        AgentCommand::Cancel(task) => {
            let (agent, task) = named(&task)?;
            AgentRequest::Cancel(agent, task)
        }
        AgentCommand::Artifact {
            target: task,
            name,
            output,
        } => return artifact(target, (named(&task)?, name), &output).await,
        AgentCommand::Define(_)
        | AgentCommand::Undefine { .. }
        | AgentCommand::Start { .. }
        | AgentCommand::Stop { .. }
        | AgentCommand::List { .. } => {
            return Err(AppError::Usage(
                "only this Runtime's own Agents are defined, started, stopped, and listed; \
                 drop --endpoint (use discover for another Endpoint's)",
            ));
        }
    };
    print(&view(
        &call(target, OpenBody::Agent(request), None).await?.1,
    ))
}

async fn call(
    target: Target<'_>,
    body: OpenBody,
    request_id: Option<&str>,
) -> Result<(Option<AgentAccept>, AgentReply), AppError> {
    let (mut reader, writer) = open_as(target.0, (target.1, request_id), body).await?;
    let answer = client::agent::call(&mut reader).await?;
    drop(writer);
    Ok(answer)
}

async fn submit(target: Target<'_>, arguments: SubmitArgs) -> Result<(), AppError> {
    let agent = agent_id(&arguments.agent)?;
    let from = arguments.from_agent.as_deref().map(agent_id).transpose()?;
    let submit = AgentSubmit::new(agent.clone(), message(&arguments.message)?, from);
    let body = OpenBody::Agent(AgentRequest::Submit(submit));
    let (accept, reply) = call(target, body, arguments.request_id.as_deref()).await?;
    let existing = matches!(accept, Some(AgentAccept::Submitted(_, true)));
    print(&json!({"task": view(&reply), "existing": existing}))?;
    let AgentReply::Summary(summary) = reply else {
        return Err(AppError::Protocol(
            "the submit answer has no task".to_owned(),
        ));
    };
    if arguments.wait {
        follow(target, (agent.clone(), summary.id), None).await?;
        let body = OpenBody::Agent(AgentRequest::Get(agent, summary.id));
        print(&view(&call(target, body, None).await?.1))?;
    }
    Ok(())
}

/// Returns whether an error only means the target cannot answer right now.
const fn transient(error: &ClientError) -> bool {
    matches!(
        error.status(),
        Some(
            ServiceError::Unavailable
                | ServiceError::Unreachable
                | ServiceError::TimedOut
                | ServiceError::OutcomeUnknown
                | ServiceError::Cancelled
        )
    )
}

/// Follows a task until it ended, printing its events after `after` when given, and
/// reconnecting while the target restarts or is unreachable.
async fn follow(
    target: Target<'_>,
    (agent, task): (AgentId, AgentTaskId),
    after: Option<u32>,
) -> Result<AgentReply, AppError> {
    let (print_events, mut seen) = (after.is_some(), after.unwrap_or(0));
    loop {
        let request = AgentRequest::Watch(agent.clone(), task, seen);
        let watched = match open(target.0, target.1, OpenBody::Agent(request)).await {
            Ok((mut reader, _writer)) => {
                let mut printed = Ok(());
                let outcome = client::agent::watch(&mut reader, |event| {
                    seen = event.seq;
                    if print_events && printed.is_ok() {
                        printed = print(&agent_event_value(&event));
                    }
                })
                .await;
                printed?;
                outcome
            }
            Err(error) => return Err(error),
        };
        match watched {
            Ok((_, reply)) => return Ok(reply),
            Err(error) if transient(&error) => tokio::time::sleep(RETRY).await,
            Err(error) => return Err(error.into()),
        }
    }
}

/// Downloads one artifact into a new local file, verified against its hash.
async fn artifact(
    target: Target<'_>,
    ((agent, task), name): ((AgentId, AgentTaskId), String),
    output: &Path,
) -> Result<(), AppError> {
    crate::commands::agent_artifact::refuse_existing(output)?;
    let request = AgentRequest::Artifact(agent, task, name.clone(), 0, None);
    let (mut reader, _writer) = open(target.0, target.1, OpenBody::Agent(request)).await?;
    let identity = client::transfer::download_accepted(&mut reader).await?;
    let source = format!("remote agent task {task} artifact {name}");
    let mut partial = client::resume::Partial::open(output, &source, false).await?;
    if let Err(error) = partial.begin(identity).await {
        let _discarded = partial.discard().await;
        return Err(error.into());
    }
    let downloaded =
        client::transfer::download_content(&mut reader, partial.sink(), (identity, 0)).await;
    let downloaded = match downloaded {
        Ok(result) => partial
            .finish_or_discard(false)
            .await
            .map(|()| result)
            .map_err(AppError::from),
        Err(error) => {
            let _discarded = partial.discard().await;
            Err(error.into())
        }
    };
    let result = downloaded?;
    print(&json!({
        "output": output.display().to_string(),
        "size": result.size,
        "blake3": result.hash.iter().fold(String::with_capacity(64), |mut hex, byte| {
            let _written = write!(hex, "{byte:02x}");
            hex
        }),
    }))
}

fn print(value: &Value) -> Result<(), AppError> {
    writeln!(std::io::stdout().lock(), "{value}")?;
    Ok(())
}
