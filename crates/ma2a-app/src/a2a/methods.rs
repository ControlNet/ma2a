//! The A2A methods answered with one JSON-RPC result.

use std::time::Duration;

use ma2a_core::service::{AgentMessage, MessagePart, MessageRole};
use serde_json::{Value, json};

use super::{
    backend::{Refusal, ended},
    mapping::Detail,
    rpc::{self, RpcError, count, required},
    view::{self, Served, settled},
};

/// Longest wait for a cancelled task to end before answering with its current state.
const CANCEL_WAIT: Duration = Duration::from_secs(30);

/// One inbound message and what it continues.
pub(super) struct Inbound {
    pub(super) message: AgentMessage,
    pub(super) message_id: String,
    /// The task it continues; the context of a task is the task.
    pub(super) task: Option<String>,
}

fn part(part: &Value) -> Result<MessagePart, RpcError> {
    if let Some(text) = part.get("text").and_then(Value::as_str) {
        return Ok(MessagePart::Text(text.to_owned()));
    }
    if let Some(data) = part.get("data") {
        return Ok(MessagePart::Data(data.to_string()));
    }
    if part.get("raw").is_some() || part.get("url").is_some() {
        return Err(RpcError::content_type(
            "file parts are not accepted; send text or data",
        ));
    }
    Err(RpcError::invalid_params(
        "a part carries text, data, raw, or url",
    ))
}

/// Reads `params.message` of `SendMessage` or `SendStreamingMessage`.
pub(super) fn inbound(params: &Value) -> Result<Inbound, RpcError> {
    let message = params
        .get("message")
        .filter(|message| message.is_object())
        .ok_or_else(|| RpcError::invalid_params("`message` is required"))?;
    let message_id = required(message, "messageId")?.to_owned();
    if message.get("role").and_then(Value::as_str) != Some("ROLE_USER") {
        return Err(RpcError::invalid_params(
            "a client message has role ROLE_USER",
        ));
    }
    let parts = message
        .get("parts")
        .and_then(Value::as_array)
        .ok_or_else(|| RpcError::invalid_params("`message.parts` is required"))?
        .iter()
        .map(part)
        .collect::<Result<Vec<_>, _>>()?;
    let parsed = AgentMessage::new(MessageRole::User, parts)
        .map_err(|_| RpcError::invalid_params("a message carries 1-16 parts within 12 KiB"))?;
    let named = |field: &str| {
        message
            .get(field)
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
    };
    let task = match (named("taskId"), named("contextId")) {
        (Some(task), Some(context)) if task != context => {
            return Err(RpcError::invalid_params("a task's contextId is its own id"));
        }
        (task, context) => task.or(context).map(str::to_owned),
    };
    Ok(Inbound {
        message: parsed,
        message_id,
        task,
    })
}

/// Creates or continues the task an inbound message names; answers its summary.
pub(super) async fn deliver(served: Served<'_>, inbound: &Inbound) -> Result<Value, Refusal> {
    let request_id = rpc::request_id(served.route, &inbound.message_id);
    if let Some(task) = &inbound.task {
        let sent = (task.as_str(), &inbound.message, request_id.as_str());
        let refusal = match served.backend.send(served.route, sent).await {
            Err(refusal) if refusal.0 == "conflict" => refusal,
            other => return other,
        };
        // The local API answers a message to an ended task as a conflict.
        let details = served.backend.details(served.route, task).await?;
        let state = details.pointer("/task/state").and_then(Value::as_str);
        if state.is_some_and(ended) {
            return Err(Refusal::named("task_ended"));
        }
        Err(refusal)
    } else {
        let submitted = (&inbound.message, request_id.as_str());
        served.backend.submit(served.route, submitted).await
    }
}

pub(super) fn task_id(summary: &Value) -> String {
    let summary = summary.get("task").unwrap_or(summary);
    summary
        .get("task_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

pub(super) fn detail(params: &Value, artifacts: bool) -> Result<Detail, RpcError> {
    Ok(Detail {
        history: count(params, "historyLength")?,
        artifacts,
    })
}

/// Answers one method that is not a stream.
pub(super) async fn call(served: Served<'_>, request: &rpc::Request) -> Result<Value, RpcError> {
    let method = request.method.as_str();
    let refused = |refusal: Refusal| RpcError::refused(method, &refusal);
    let params = &request.params;
    match method {
        "SendMessage" => {
            let inbound = inbound(params)?;
            let configuration = params.get("configuration").unwrap_or(&Value::Null);
            let summary = deliver(served, &inbound).await.map_err(refused)?;
            if configuration
                .get("returnImmediately")
                .and_then(Value::as_bool)
                != Some(true)
            {
                // A message to a waiting task counts only once the Agent took it.
                let fresh = inbound.task.is_some();
                view::wait(served, (&summary, fresh), settled)
                    .await
                    .map_err(refused)?;
            }
            let detail = detail(configuration, true)?;
            let task = view::task(served, &task_id(&summary), detail)
                .await
                .map_err(refused)?;
            Ok(json!({"task": task}))
        }
        "GetTask" => {
            let id = required(params, "id")?;
            view::task(served, id, detail(params, true)?)
                .await
                .map_err(refused)
        }
        "CancelTask" => {
            let id = required(params, "id")?;
            let summary = served
                .backend
                .cancel(served.route, id)
                .await
                .map_err(refused)?;
            let waited =
                tokio::time::timeout(CANCEL_WAIT, view::wait(served, (&summary, false), ended));
            if let Ok(waited) = waited.await {
                waited.map_err(refused)?;
            }
            view::task(served, id, detail(params, true)?)
                .await
                .map_err(refused)
        }
        "ListTasks" => super::list::list(served, params).await,
        method if method.contains("PushNotificationConfig") => Err(RpcError::push_not_supported()),
        "GetExtendedAgentCard" => Err(RpcError::no_extended_card()),
        _ => Err(RpcError::method_not_found()),
    }
}
