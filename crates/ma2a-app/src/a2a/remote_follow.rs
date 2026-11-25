//! Following another Endpoint's task with `watch`, across interruptions of the target.

use std::time::Duration;

use ma2a_core::service::{AgentEvent, AgentEventBody, AgentRequest, OpenBody};
use ma2a_runtime::api::agent_event_value;
use ma2a_services::client;
use serde_json::Value;
use tokio::sync::mpsc;

use super::{parts, refusal, task_id};
use crate::{
    a2a::backend::{Backend, Refusal, Route, message_json},
    commands::remote::{
        agent::{passes, reopening_passes},
        open_as, own_endpoint,
    },
};

/// Pause before reaching a target again that is restarting or unreachable.
const RETRY: Duration = Duration::from_secs(1);

/// The local API view of one event with its message in full. The view leaves out a message
/// too large for a local response, but here the typed event is already all here.
fn in_full(event: &AgentEvent) -> Value {
    let mut value = agent_event_value(event);
    if let (AgentEventBody::Message(message), Some(fields)) = (&event.body, value.as_object_mut()) {
        fields.insert("message".to_owned(), message_json(message));
        fields.insert("message_elided".to_owned(), Value::Bool(false));
    }
    value
}

/// Follows a task until it ended, reconnecting while the target restarts; stops early
/// once nobody receives `events` any more.
///
/// Once a watch was interrupted, reopening it can fail the same passing way, for example while
/// this Runtime restarts, so that is retried too.
pub(in crate::a2a) async fn follow(
    backend: &Backend,
    (route, (task, mut after)): (&Route, (&str, u32)),
    events: &mpsc::UnboundedSender<Value>,
) -> Result<(), Refusal> {
    let (endpoint, agent) = parts(route)?;
    let task = task_id(task)?;
    let caller = own_endpoint(&backend.paths).await?;
    // The task's end once taken, which a reconnected watch must end with and nothing follow.
    let mut ended = None;
    let mut reconnecting = false;
    loop {
        let request = OpenBody::Agent(AgentRequest::Watch(agent.clone(), task, after));
        let refused = match open_as(&backend.paths, (endpoint, None), request).await {
            Ok((mut reader, _writer)) => {
                let watching = client::agent::watch(
                    &mut reader,
                    ((&caller, agent, task), (after, ended)),
                    |event| {
                        (after, ended) = (event.seq, client::agent::end_of(&event));
                        let _sent = events.send(in_full(&event));
                    },
                );
                let watched = tokio::select! {
                    watched = watching => watched,
                    () = events.closed() => return Ok(()),
                };
                match watched {
                    Ok(_) => return Ok(()),
                    Err(error) => (true, refusal(&error)),
                }
            }
            Err(error) => (
                reconnecting && reopening_passes(&error),
                Refusal::from(error),
            ),
        };
        match refused {
            (true, refusal) if passes(&refusal.0) => reconnecting = true,
            (_, refusal) => return Err(refusal),
        }
        tokio::select! {
            () = tokio::time::sleep(RETRY) => {}
            () = events.closed() => return Ok(()),
        }
    }
}
