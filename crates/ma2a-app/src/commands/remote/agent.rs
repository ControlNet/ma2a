//! `ma2a agent --endpoint`: another Endpoint's Agents, used as this Endpoint.

use std::{fmt::Write as _, io::Write as _, path::Path, time::Duration};

use ma2a_core::service::{
    AgentAccept, AgentId, AgentMessage, AgentReply, AgentRequest, AgentSubmit, AgentTaskId,
    MessagePart, MessageRole, OpenBody, ServiceError,
};
use ma2a_runtime::{api::agent_event_value, ipc::IpcPaths};
use ma2a_services::client;
use serde_json::{Value, json};

use super::{READ_DEADLINE, agent_view::view, bounded, open, open_bound, own_endpoint};
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
            let request = AgentRequest::Send(agent, task, message(&parts)?);
            return print(&view(
                &call(target, request, request_id.as_deref()).await?.1,
            ));
        }
        AgentCommand::Cancel(task) => {
            let (agent, task) = named(&task)?;
            let request = AgentRequest::Cancel(agent, task);
            return print(&view(&call(target, request, None).await?.1));
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
    print(&view(&read(target, request).await?))
}

/// Runs one read-only operation, which gives up on a target that does not answer in time.
async fn read(target: Target<'_>, request: AgentRequest) -> Result<AgentReply, AppError> {
    bounded(READ_DEADLINE, async {
        Ok(call(target, request, None).await?.1)
    })
    .await
}

/// Runs one content-free operation; its answer must be the one `request` asked for, never
/// another task's or another Agent's.
async fn call(
    target: Target<'_>,
    request: AgentRequest,
    request_id: Option<&str>,
) -> Result<(Option<AgentAccept>, AgentReply), AppError> {
    let caller = own_endpoint(target.0).await?;
    let body = OpenBody::Agent(request.clone());
    let (opened, (mut reader, writer)) = open_bound(target.0, (target.1, request_id), body).await?;
    let (accept, reply) = client::agent::call(&mut reader).await?;
    drop(writer);
    if !client::answer::agent((&request, &opened, &caller), (accept, &reply)) {
        return Err(AppError::Protocol(
            "the target answered another task or operation".to_owned(),
        ));
    }
    Ok((accept, reply))
}

async fn submit(target: Target<'_>, arguments: SubmitArgs) -> Result<(), AppError> {
    let agent = agent_id(&arguments.agent)?;
    let from = arguments.from_agent.as_deref().map(agent_id).transpose()?;
    let submit = AgentSubmit::new(agent.clone(), message(&arguments.message)?, from);
    let request = AgentRequest::Submit(submit);
    let (accept, reply) = call(target, request, arguments.request_id.as_deref()).await?;
    // The answer was checked to be the Summary of the task the submit was bound to.
    let AgentReply::Summary(summary) = &reply else {
        return Err(AppError::Protocol(
            "the submit answer has no task".to_owned(),
        ));
    };
    let existing = matches!(accept, Some(AgentAccept::Submitted(_, true)));
    print(&json!({"task": view(&reply), "existing": existing}))?;
    if arguments.wait {
        follow(target, (agent.clone(), summary.id), None).await?;
        print(&view(
            &read(target, AgentRequest::Get(agent, summary.id)).await?,
        ))?;
    }
    Ok(())
}

/// Returns whether the status `name` only means the target cannot answer right now: it is
/// restarting or out of reach.
pub(crate) fn passes(name: &str) -> bool {
    [
        ServiceError::Unavailable,
        ServiceError::Unreachable,
        ServiceError::TimedOut,
        ServiceError::OutcomeUnknown,
        ServiceError::Cancelled,
    ]
    .iter()
    .any(|status| status.name() == name)
}

/// Returns whether reopening an interrupted watch failed in a way that passes: the Runtime
/// keeps the meaning of its own refusals, and any other local failure is this Runtime being
/// away for now, as while it restarts.
fn reopening_passes(error: &AppError) -> bool {
    match error {
        AppError::Rejected(name, _) => passes(name),
        _ => true,
    }
}

/// Follows a task until it ended, printing its events after `after` when given, and
/// reconnecting while the target restarts or is unreachable. Once a watch was interrupted,
/// reopening it can fail the same passing way, so that is retried too.
async fn follow(
    target: Target<'_>,
    (agent, task): (AgentId, AgentTaskId),
    after: Option<u32>,
) -> Result<AgentReply, AppError> {
    let (print_events, mut seen) = (after.is_some(), after.unwrap_or(0));
    let mut reconnecting = false;
    loop {
        let request = AgentRequest::Watch(agent.clone(), task, seen);
        let watched = match open(target.0, target.1, OpenBody::Agent(request)).await {
            Ok((mut reader, _writer)) => {
                let mut printed = Ok(());
                let outcome = client::agent::watch(&mut reader, (&agent, task, seen), |event| {
                    seen = event.seq;
                    if print_events && printed.is_ok() {
                        printed = print(&agent_event_value(&event));
                    }
                })
                .await;
                printed?;
                outcome
            }
            Err(error) if reconnecting && reopening_passes(&error) => {
                tokio::time::sleep(RETRY).await;
                continue;
            }
            Err(error) => return Err(error),
        };
        match watched {
            Ok((_, reply)) => return Ok(reply),
            Err(error) if error.status().is_some_and(|status| passes(status.name())) => {
                reconnecting = true;
                tokio::time::sleep(RETRY).await;
            }
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
    // Only the answer is bounded; content may take as long as it takes to arrive.
    let (mut reader, _writer, identity) = bounded(READ_DEADLINE, async {
        let (mut reader, writer) = open(target.0, target.1, OpenBody::Agent(request)).await?;
        let identity = client::transfer::download_accepted(&mut reader).await?;
        Ok((reader, writer, identity))
    })
    .await?;
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
