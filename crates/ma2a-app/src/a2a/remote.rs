//! Another Endpoint's Agent, through this Runtime's stream plane as this Endpoint.

use std::time::Duration;

use ma2a_core::service::{
    AgentId, AgentMessage, AgentReply, AgentRequest, AgentSubmit, AgentTaskId, OpenBody,
    ServiceError,
};
use ma2a_runtime::api::{agent_event_value, agent_task_details_value, agent_task_value};
use ma2a_services::client::{self, ClientError};
use serde_json::Value;
use tokio::{io::AsyncWrite, sync::mpsc};

use super::backend::{Backend, Refusal, Route};
use crate::commands::remote::open_as;

/// Pause before reaching a target again that is restarting or unreachable.
const RETRY: Duration = Duration::from_secs(1);

fn refusal(error: &ClientError) -> Refusal {
    Refusal::named(error.status().map_or("unavailable", ServiceError::name))
}

fn parts(route: &Route) -> Result<(&str, &AgentId), Refusal> {
    match route {
        Route::Remote(endpoint, agent) => Ok((endpoint, agent)),
        Route::Local(_) => Err(Refusal::named("internal")),
    }
}

fn task_id(task: &str) -> Result<AgentTaskId, Refusal> {
    AgentTaskId::parse(task).map_err(|_| Refusal::named("unknown_task"))
}

/// Runs one content-free operation and answers its reply.
async fn call(
    backend: &Backend,
    (endpoint, request_id): (&str, Option<&str>),
    request: AgentRequest,
) -> Result<AgentReply, Refusal> {
    let (mut reader, writer) = open_as(
        &backend.paths,
        (endpoint, request_id),
        OpenBody::Agent(request),
    )
    .await?;
    let answer = client::agent::call(&mut reader).await;
    drop(writer);
    answer
        .map(|(_, reply)| reply)
        .map_err(|error| refusal(&error))
}

fn summary(reply: &AgentReply) -> Result<Value, Refusal> {
    match reply {
        AgentReply::Summary(summary) => Ok(agent_task_value(summary)),
        AgentReply::Details(details) => Ok(agent_task_details_value(details)),
        AgentReply::Cards(..) | AgentReply::Page(..) => Err(Refusal::named("internal")),
    }
}

pub(super) async fn submit(
    backend: &Backend,
    route: &Route,
    (message, request_id): (&AgentMessage, &str),
) -> Result<Value, Refusal> {
    let (endpoint, agent) = parts(route)?;
    let submit = AgentSubmit::new(agent.clone(), message.clone(), None);
    let request = AgentRequest::Submit(submit);
    summary(&call(backend, (endpoint, Some(request_id)), request).await?)
}

pub(super) async fn send(
    backend: &Backend,
    route: &Route,
    (task, message, request_id): (&str, &AgentMessage, &str),
) -> Result<Value, Refusal> {
    let (endpoint, agent) = parts(route)?;
    let request = AgentRequest::Send(agent.clone(), task_id(task)?, message.clone());
    summary(&call(backend, (endpoint, Some(request_id)), request).await?)
}

pub(super) async fn details(
    backend: &Backend,
    route: &Route,
    task: &str,
) -> Result<Value, Refusal> {
    let (endpoint, agent) = parts(route)?;
    let request = AgentRequest::Get(agent.clone(), task_id(task)?);
    summary(&call(backend, (endpoint, None), request).await?)
}

pub(super) async fn tasks(
    backend: &Backend,
    route: &Route,
    after: Option<&str>,
) -> Result<(Vec<Value>, Option<String>), Refusal> {
    let (endpoint, agent) = parts(route)?;
    let after = after.map(task_id).transpose()?;
    match call(
        backend,
        (endpoint, None),
        AgentRequest::Tasks(agent.clone(), after),
    )
    .await?
    {
        AgentReply::Page(tasks, next) => Ok((
            tasks.iter().map(agent_task_value).collect(),
            next.map(|task| task.to_string()),
        )),
        _ => Err(Refusal::named("internal")),
    }
}

/// Cancels a task; the target answers once it ended.
pub(super) async fn cancel(backend: &Backend, route: &Route, task: &str) -> Result<Value, Refusal> {
    let (endpoint, agent) = parts(route)?;
    let request = AgentRequest::Cancel(agent.clone(), task_id(task)?);
    summary(&call(backend, (endpoint, None), request).await?)
}

pub(super) async fn card(backend: &Backend, route: &Route) -> Result<(String, bool), Refusal> {
    let (endpoint, agent) = parts(route)?;
    let mut after = None;
    loop {
        let request = AgentRequest::Agents(after.take());
        let AgentReply::Cards(cards, next) = call(backend, (endpoint, None), request).await? else {
            return Err(Refusal::named("internal"));
        };
        if let Some(card) = cards.iter().find(|card| card.id == *agent) {
            return Ok((card.description.clone(), card.available));
        }
        after = Some(next.ok_or_else(|| Refusal::named("unknown_agent"))?);
    }
}

/// Follows a task until it ended, reconnecting while the target restarts; stops early
/// once nobody receives `events` any more.
pub(super) async fn follow(
    backend: &Backend,
    (route, (task, mut after)): (&Route, (&str, u32)),
    events: &mpsc::UnboundedSender<Value>,
) -> Result<(), Refusal> {
    let (endpoint, agent) = parts(route)?;
    let task = task_id(task)?;
    loop {
        let request = OpenBody::Agent(AgentRequest::Watch(agent.clone(), task, after));
        let (mut reader, _writer) = open_as(&backend.paths, (endpoint, None), request).await?;
        let watching = client::agent::watch(&mut reader, |event| {
            after = event.seq;
            let _sent = events.send(agent_event_value(&event));
        });
        let watched = tokio::select! {
            watched = watching => watched,
            () = events.closed() => return Ok(()),
        };
        match watched {
            Ok(_) => return Ok(()),
            Err(error) if is_transient(&error) => tokio::time::sleep(RETRY).await,
            Err(error) => return Err(refusal(&error)),
        }
    }
}

const fn is_transient(error: &ClientError) -> bool {
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

/// Streams one artifact's verified content into `sink`.
pub(super) async fn artifact<S: AsyncWrite + Unpin>(
    backend: &Backend,
    (route, task, name): (&Route, &str, &str),
    sink: &mut S,
) -> Result<(), Refusal> {
    let (endpoint, agent) = parts(route)?;
    let request = AgentRequest::Artifact(agent.clone(), task_id(task)?, name.to_owned(), 0, None);
    let (mut reader, _writer) =
        open_as(&backend.paths, (endpoint, None), OpenBody::Agent(request)).await?;
    let identity = client::transfer::download_accepted(&mut reader)
        .await
        .map_err(|error| refusal(&error))?;
    let mut hasher = blake3::Hasher::new();
    client::transfer::download_content(&mut reader, (sink, &mut hasher), (identity, 0))
        .await
        .map_err(|error| refusal(&error))?;
    Ok(())
}
