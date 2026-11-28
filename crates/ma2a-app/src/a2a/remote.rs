//! Another Endpoint's Agent, through this Runtime's stream plane as this Endpoint.

use std::time::Duration;

use ma2a_core::service::{
    AgentAccept, AgentId, AgentMessage, AgentReply, AgentRequest, AgentSubmit, AgentTaskDetails,
    AgentTaskId, OpenBody, ServiceError,
};
use ma2a_runtime::api::{agent_task_details_value, agent_task_value};
use ma2a_services::client::{self, ClientError};
use serde_json::Value;

use super::backend::{Backend, Refusal, Route, message_json};
use crate::commands::remote::{open_bound, own_endpoint};

pub(super) fn refusal(error: &ClientError) -> Refusal {
    match error {
        // A target that broke the contract refused nothing the client sent.
        ClientError::Stream(ServiceError::InvalidInput) => Refusal::named("internal"),
        _ => Refusal::named(error.status().map_or("unavailable", ServiceError::name)),
    }
}

pub(super) fn parts(route: &Route) -> Result<(&str, &AgentId), Refusal> {
    match route {
        Route::Remote(endpoint, agent) => Ok((endpoint, agent)),
        Route::Local(_) => Err(Refusal::named("internal")),
    }
}

pub(super) fn task_id(task: &str) -> Result<AgentTaskId, Refusal> {
    AgentTaskId::parse(task).map_err(|_| Refusal::named("unknown_task"))
}

/// Longest wait for a target to answer one content-free operation, so a target that never
/// answers fails the request instead of holding one of the adapter's connections.
pub(super) const CALL_DEADLINE: Duration = Duration::from_secs(30);

/// Runs `call` under `deadline`, failing as `timed_out` once it passes.
pub(super) async fn bounded<T>(
    deadline: Duration,
    call: impl Future<Output = Result<T, Refusal>>,
) -> Result<T, Refusal> {
    tokio::time::timeout(deadline, call)
        .await
        .unwrap_or_else(|_| Err(Refusal::named(ServiceError::TimedOut.name())))
}

/// What a target answered one content-free operation: its accept, unless the outcome came
/// from caller-side replay, and its reply.
type Answer = (Option<AgentAccept>, AgentReply);

/// Runs one content-free operation under [`CALL_DEADLINE`] and answers what the target did.
async fn call(
    backend: &Backend,
    target: (&str, Option<&str>),
    request: AgentRequest,
) -> Result<Answer, Refusal> {
    bounded(CALL_DEADLINE, answer(backend, target, request)).await
}

/// Runs one content-free operation, however long its target takes, and answers what the
/// target did, once it is an answer to that operation (see [`client::answer::agent`]): a
/// `submit` Summary of the task its own `RequestId` created, the Summary of a `cancel` once the
/// task ended, and only a mutation answered from caller-side replay.
async fn answer(
    backend: &Backend,
    (endpoint, request_id): (&str, Option<&str>),
    request: AgentRequest,
) -> Result<Answer, Refusal> {
    let caller = own_endpoint(&backend.paths).await?;
    let (opened, (mut reader, writer)) = open_bound(
        &backend.paths,
        (endpoint, request_id),
        OpenBody::Agent(request.clone()),
    )
    .await?;
    let answer = client::agent::call(&mut reader).await;
    drop(writer);
    let (accept, reply) = answer.map_err(|error| refusal(&error))?;
    if !client::answer::agent((&request, &opened, &caller), (accept, &reply)) {
        return Err(Refusal::named("internal"));
    }
    Ok((accept, reply))
}

/// The task one operation answered, which must be what it asked for: the Details of `get`,
/// or the Summary of any other, of the route's Agent and, once known, of the task named.
/// Anything else is a malformed answer, never another task passed off as this one.
fn outcome(
    reply: &AgentReply,
    (agent, task): (&AgentId, Option<AgentTaskId>),
    details: bool,
) -> Result<Value, Refusal> {
    let (summary, value) = match (reply, details) {
        (AgentReply::Summary(summary), false) => (summary, agent_task_value(summary)),
        (AgentReply::Details(full), true) => (&full.summary, in_full(full)),
        _ => return Err(Refusal::named("internal")),
    };
    if summary.agent != *agent || task.is_some_and(|task| task != summary.id) {
        return Err(Refusal::named("internal"));
    }
    Ok(value)
}

/// The details of a task with its messages in full. The local API's view leaves out a message
/// too large for a local response, but here the typed answer is already all here.
fn in_full(details: &AgentTaskDetails) -> Value {
    let mut value = agent_task_details_value(details);
    if let Some(fields) = value.as_object_mut() {
        let result = details.result.as_ref().map_or(Value::Null, message_json);
        fields.insert("input".to_owned(), message_json(&details.input));
        fields.insert("result".to_owned(), result);
        fields.insert("input_elided".to_owned(), Value::Bool(false));
        fields.insert("result_elided".to_owned(), Value::Bool(false));
    }
    value
}

pub(super) async fn submit(
    backend: &Backend,
    route: &Route,
    (message, request_id): (&AgentMessage, &str),
) -> Result<Value, Refusal> {
    let (endpoint, agent) = parts(route)?;
    let submit = AgentSubmit::new(agent.clone(), message.clone(), None);
    let request = AgentRequest::Submit(submit);
    let (accept, reply) = call(backend, (endpoint, Some(request_id)), request).await?;
    // A replayed outcome has no accept; a fresh one names the task it was bound to.
    let task = match accept {
        Some(AgentAccept::Submitted(task, _)) => Some(task),
        _ => None,
    };
    outcome(&reply, (agent, task), false)
}

pub(super) async fn send(
    backend: &Backend,
    route: &Route,
    (task, message, request_id): (&str, &AgentMessage, &str),
) -> Result<Value, Refusal> {
    let (endpoint, agent) = parts(route)?;
    let task = task_id(task)?;
    let request = AgentRequest::Send(agent.clone(), task, message.clone());
    let (_, reply) = call(backend, (endpoint, Some(request_id)), request).await?;
    outcome(&reply, (agent, Some(task)), false)
}

pub(super) async fn details(
    backend: &Backend,
    route: &Route,
    task: &str,
) -> Result<Value, Refusal> {
    let (endpoint, agent) = parts(route)?;
    let task = task_id(task)?;
    let (_, reply) = call(
        backend,
        (endpoint, None),
        AgentRequest::Get(agent.clone(), task),
    )
    .await?;
    outcome(&reply, (agent, Some(task)), true)
}

/// Whether a page lists tasks or Agents in ascending order strictly after the cursor sent, and
/// names its own last one as the cursor when more exist. A target answering otherwise would
/// keep a listing going forever, or put an entry where no cursor reaches it.
fn moves_past<T: Ord>(
    after: Option<&T>,
    (listed, next): (impl Iterator<Item = T>, Option<T>),
) -> bool {
    let mut last = None;
    for entry in listed {
        if last.as_ref().or(after).is_some_and(|floor| entry <= *floor) {
            return false;
        }
        last = Some(entry);
    }
    next.is_none_or(|next| last == Some(next))
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
    .1
    {
        // A page names only the route's Agent's tasks, so no other is listed as its own.
        AgentReply::Page(tasks, next)
            if moves_past(after.as_ref(), (tasks.iter().map(|task| task.id), next))
                && tasks.iter().all(|task| task.agent == *agent) =>
        {
            Ok((
                tasks.iter().map(agent_task_value).collect(),
                next.map(|task| task.to_string()),
            ))
        }
        _ => Err(Refusal::named("internal")),
    }
}

/// Cancels a task; the target answers once it ended.
pub(super) async fn cancel(backend: &Backend, route: &Route, task: &str) -> Result<Value, Refusal> {
    let (endpoint, agent) = parts(route)?;
    let task = task_id(task)?;
    let request = AgentRequest::Cancel(agent.clone(), task);
    // `CancelTask` bounds the cancel by its own wait and then answers the task as it is, so
    // a deadline here would only turn that answer into a failure.
    let (_, reply) = answer(backend, (endpoint, None), request).await?;
    outcome(&reply, (agent, Some(task)), false)
}

pub(super) async fn card(backend: &Backend, route: &Route) -> Result<(String, bool), Refusal> {
    let (endpoint, agent) = parts(route)?;
    let mut after = None;
    for _ in 0..MAX_CARD_PAGES {
        let request = AgentRequest::Agents(after.clone());
        let (_, AgentReply::Cards(cards, next)) = call(backend, (endpoint, None), request).await?
        else {
            return Err(Refusal::named("internal"));
        };
        // A target that does not move past the cursor sent is not followed.
        if !moves_past(
            after.as_ref(),
            (cards.iter().map(|card| card.id.clone()), next.clone()),
        ) {
            return Err(Refusal::named("internal"));
        }
        if let Some(card) = cards.iter().find(|card| card.id == *agent) {
            return Ok((card.description.clone(), card.available));
        }
        after = Some(next.ok_or_else(|| Refusal::named("unknown_agent"))?);
    }
    // A target still paging after that many pages is not believed.
    Err(Refusal::named("internal"))
}

/// Most Agent card pages read to find one Agent: 1,024 Agents, more than a Runtime is expected
/// to host.
const MAX_CARD_PAGES: usize = 64;

#[path = "remote_follow.rs"]
mod following;
pub(super) use following::follow;

#[cfg(test)]
#[path = "remote_tests.rs"]
mod tests;

#[cfg(all(test, unix))]
#[path = "remote_answer_tests.rs"]
mod answer_tests;
