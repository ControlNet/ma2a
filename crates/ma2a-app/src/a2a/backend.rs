//! The Agents an A2A route names, reached as this Runtime's local operator or, for another
//! Endpoint's, through the stream plane as this Endpoint.
//!
//! Every answer is in the local API's shape, whichever way it was reached.

use std::{path::PathBuf, time::Duration};

use ma2a_core::service::{AgentId, AgentMessage, AgentTaskId, MessagePart};
use ma2a_runtime::{
    api::{AGENT_UNAVAILABLE_REMEDIATION, TASK_ENDED_REMEDIATION},
    ipc::IpcPaths,
};
use serde_json::{Map, Value, json};
use tokio::sync::mpsc;

use crate::commands::workflows::{command, request_fields};

/// Pause between two reads of a local task that has not ended yet.
const POLL: Duration = Duration::from_millis(250);

/// Where one A2A route's Agent lives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Route {
    /// An Agent this Runtime hosts, used as its local operator.
    Local(AgentId),
    /// Another Endpoint's Agent, used as this Endpoint.
    Remote(String, AgentId),
}

impl Route {
    pub(super) const fn agent(&self) -> &AgentId {
        match self {
            Self::Local(agent) | Self::Remote(_, agent) => agent,
        }
    }

    /// The URL path of this route, without a trailing slash.
    pub(super) fn path(&self) -> String {
        match self {
            Self::Local(agent) => format!("/agents/{}", agent.as_str()),
            Self::Remote(endpoint, agent) => {
                format!("/endpoints/{endpoint}/agents/{}", agent.as_str())
            }
        }
    }
}

/// Why the Runtime or the target Endpoint refused: a local API or Agent v1 error name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Refusal(pub(super) String);

impl Refusal {
    pub(super) fn named(name: &str) -> Self {
        Self(name.to_owned())
    }
}

impl From<crate::AppError> for Refusal {
    fn from(_: crate::AppError) -> Self {
        Self::named("unavailable")
    }
}

/// The Runtime the adapter is a client of.
#[derive(Clone, Debug)]
pub(super) struct Backend {
    pub(super) state_dir: PathBuf,
    pub(super) paths: IpcPaths,
}

/// The local API form of one message.
pub(super) fn message_json(message: &AgentMessage) -> Value {
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

/// Returns whether a local API task state is terminal.
pub(super) fn ended(state: &str) -> bool {
    matches!(state, "completed" | "failed" | "cancelled")
}

impl Backend {
    /// Runs one local API command as this Runtime's operator and answers its payload.
    pub(super) async fn local(
        &self,
        operation: &str,
        fields: Map<String, Value>,
    ) -> Result<Value, Refusal> {
        let request = command(operation, fields)?;
        let response = crate::request((&self.state_dir, self.paths.clone()), request).await?;
        let document: Value =
            serde_json::from_slice(&response).map_err(|_| Refusal::named("internal"))?;
        if let Some(refusal) = refused(&document) {
            return Err(refusal);
        }
        Ok(document
            .pointer("/result/payload")
            .cloned()
            .unwrap_or(Value::Null))
    }

    /// Creates a task under `request_id`; answers the task summary.
    pub(super) async fn submit(
        &self,
        route: &Route,
        (message, request_id): (&AgentMessage, &str),
    ) -> Result<Value, Refusal> {
        let Route::Local(agent) = route else {
            return super::remote::submit(self, route, (message, request_id)).await;
        };
        let fields = fields([
            ("request_id", json!(request_id)),
            ("agent", json!(agent.as_str())),
            ("message", message_json(message)),
        ]);
        let submitted = self.local("agent_task_submit", fields).await?;
        Ok(submitted.get("task").cloned().unwrap_or(Value::Null))
    }

    /// Sends the next message of a waiting task under `request_id`.
    pub(super) async fn send(
        &self,
        route: &Route,
        (task, message, request_id): (&str, &AgentMessage, &str),
    ) -> Result<Value, Refusal> {
        if matches!(route, Route::Remote(..)) {
            return super::remote::send(self, route, (task, message, request_id)).await;
        }
        // The operator may send to any task; the route's Agent must own it.
        self.details(route, task).await?;
        let fields = fields([
            ("request_id", json!(request_id)),
            ("task_id", json!(task)),
            ("message", message_json(message)),
        ]);
        self.local("agent_task_send", fields).await
    }

    /// One task in full, as the route's Agent's.
    pub(super) async fn details(&self, route: &Route, task: &str) -> Result<Value, Refusal> {
        let Route::Local(agent) = route else {
            return super::remote::details(self, route, task).await;
        };
        // A task ID the local API cannot read names no task, as it does on a remote route.
        if AgentTaskId::parse(task).is_err() {
            return Err(Refusal::named("not_found"));
        }
        let details = self
            .local("agent_task_inspect", fields([("task_id", json!(task))]))
            .await?;
        if details.pointer("/task/agent").and_then(Value::as_str) == Some(agent.as_str()) {
            Ok(details)
        } else {
            Err(Refusal::named("not_found"))
        }
    }

    /// One page of the route's Agent's tasks after `after`, and where the next begins.
    pub(super) async fn tasks(
        &self,
        route: &Route,
        after: Option<&str>,
    ) -> Result<(Vec<Value>, Option<String>), Refusal> {
        let Route::Local(agent) = route else {
            return super::remote::tasks(self, route, after).await;
        };
        let fields = fields([("agent", json!(agent.as_str())), ("after", json!(after))]);
        let page = self.local("agent_task_list", fields).await?;
        let tasks = page
            .get("tasks")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let next = page.get("next").and_then(Value::as_str).map(str::to_owned);
        Ok((tasks, next))
    }

    /// Requests a task's cancellation and answers its summary.
    pub(super) async fn cancel(&self, route: &Route, task: &str) -> Result<Value, Refusal> {
        if matches!(route, Route::Remote(..)) {
            return super::remote::cancel(self, route, task).await;
        }
        self.details(route, task).await?;
        let mut fields = request_fields()?;
        fields.insert("task_id".to_owned(), json!(task));
        self.local("agent_task_cancel", fields).await
    }

    /// The route's Agent's description and whether it is started.
    pub(super) async fn card(&self, route: &Route) -> Result<(String, bool), Refusal> {
        let Route::Local(agent) = route else {
            return super::remote::card(self, route).await;
        };
        let mut after: Option<String> = None;
        loop {
            let page = self
                .local("agent_list", fields([("after", json!(after))]))
                .await?;
            let agents = page.get("agents").and_then(Value::as_array);
            if let Some(found) = agents
                .into_iter()
                .flatten()
                .find(|entry| entry.get("agent").and_then(Value::as_str) == Some(agent.as_str()))
            {
                let description = found.get("description").and_then(Value::as_str);
                let enabled = found.get("enabled").and_then(Value::as_bool);
                return Ok((
                    description.unwrap_or_default().to_owned(),
                    enabled == Some(true),
                ));
            }
            after = page.get("next").and_then(Value::as_str).map(str::to_owned);
            if after.is_none() {
                return Err(Refusal::named("not_found"));
            }
        }
    }

    /// Sends every event of a task after `after` to `events`, in order, until it ended or
    /// nobody receives them any more.
    pub(super) async fn follow(
        &self,
        (route, (task, mut after)): (&Route, (&str, u32)),
        events: &mpsc::UnboundedSender<Value>,
    ) -> Result<(), Refusal> {
        if matches!(route, Route::Remote(..)) {
            return super::remote::follow(self, (route, (task, after)), events).await;
        }
        loop {
            // The state is read before the events: once it is terminal, every event is there.
            let details = self.details(route, task).await?;
            let state = details.pointer("/task/state").and_then(Value::as_str);
            let mut more = true;
            while more {
                let fields = fields([("task_id", json!(task)), ("after", json!(after))]);
                let page = self.local("agent_task_events", fields).await?;
                for event in page
                    .get("events")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    let seq = event.get("seq").and_then(Value::as_u64);
                    after = seq.and_then(|seq| u32::try_from(seq).ok()).unwrap_or(after);
                    if events.send(event.clone()).is_err() {
                        return Ok(());
                    }
                }
                more = page.get("more").and_then(Value::as_bool).unwrap_or(false);
            }
            if state.is_some_and(ended) || events.is_closed() {
                return Ok(());
            }
            tokio::time::sleep(POLL).await;
        }
    }
}

/// The refusal a local API error envelope carries, if it is one. A message to a task that
/// has ended or is being cancelled, an Agent that is not started, and a reused request ID
/// all answer `conflict`; the first two carry their own remediation, so they become
/// `task_ended` and `agent_unavailable`.
fn refused(document: &Value) -> Option<Refusal> {
    let error = document.get("error").and_then(Value::as_str)?;
    let remediation = document.get("remediation").and_then(Value::as_str);
    Some(Refusal::named(match (error, remediation) {
        ("conflict", Some(TASK_ENDED_REMEDIATION)) => "task_ended",
        ("conflict", Some(AGENT_UNAVAILABLE_REMEDIATION)) => "agent_unavailable",
        _ => error,
    }))
}

/// Builds one local API field map.
pub(super) fn fields<const N: usize>(entries: [(&str, Value); N]) -> Map<String, Value> {
    entries
        .into_iter()
        .map(|(name, value)| (name.to_owned(), value))
        .collect()
}

#[cfg(test)]
#[path = "backend_tests.rs"]
mod tests;
