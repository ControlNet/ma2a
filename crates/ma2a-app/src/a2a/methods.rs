//! The A2A methods answered with one JSON-RPC result.

use std::time::Duration;

use ma2a_core::service::{AgentMessage, MessagePart, MessageRole};
use serde_json::{Value, json};

use super::{
    backend::{Refusal, ended},
    mapping::Detail,
    rpc::{self, RpcError, count, flag, optional, required},
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
    let file = |name: &str| part.get(name).is_some_and(|value| !value.is_null());
    if file("raw") || file("url") {
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
    // A task ID that is not a string is refused, not read as absent, which would start a task.
    let task = match (
        optional(message, "taskId")?,
        optional(message, "contextId")?,
    ) {
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
        match served.backend.send(served.route, sent).await {
            // The local API answers a task it does not know as `not_found`.
            Err(refusal) if refusal.0 == "not_found" => Err(Refusal::named("unknown_task")),
            other => other,
        }
    } else {
        let submitted = (&inbound.message, request_id.as_str());
        match served.backend.submit(served.route, submitted).await {
            // The local API answers a stopped Agent as a conflict, like a reused messageId.
            Err(refusal) if refusal.0 == "conflict" => {
                let started = served.backend.card(served.route).await;
                if started.is_ok_and(|(_, started)| !started) {
                    return Err(Refusal::named("agent_unavailable"));
                }
                Err(refusal)
            }
            other => other,
        }
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

/// The `configuration` of `SendMessage` or `SendStreamingMessage`: whether to answer at once,
/// and the detail it asks for. Both read all of it before delivering anything, so a refusal
/// never follows a task it already created or a message it already sent.
pub(super) fn configuration(params: &Value) -> Result<(bool, Detail), RpcError> {
    let configuration = match params.get("configuration") {
        None | Some(Value::Null) => &Value::Null,
        Some(object @ Value::Object(_)) => object,
        Some(_) => return Err(RpcError::invalid_params("`configuration` is an object")),
    };
    Ok((
        flag(configuration, "returnImmediately")?,
        detail(configuration, true)?,
    ))
}

/// Answers one method that is not a stream.
pub(super) async fn call(served: Served<'_>, request: &rpc::Request) -> Result<Value, RpcError> {
    let method = request.method.as_str();
    let refused = |refusal: Refusal| RpcError::refused(method, &refusal);
    let params = &request.params;
    match method {
        "SendMessage" => {
            let inbound = inbound(params)?;
            let (immediately, detail) = configuration(params)?;
            let summary = deliver(served, &inbound).await.map_err(refused)?;
            if !immediately {
                // A message to a waiting task counts only once the Agent took it.
                let fresh = inbound.task.is_some();
                view::wait(served, (&summary, fresh), settled)
                    .await
                    .map_err(refused)?;
            }
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
            let detail = detail(params, true)?;
            let current = served
                .backend
                .details(served.route, id)
                .await
                .map_err(refused)?;
            let state = current.pointer("/task/state").and_then(Value::as_str);
            if state.is_some_and(ended) {
                return Err(RpcError::not_cancelable());
            }
            // The wait covers the cancel itself: a remote target answers it only once the task
            // ended, so past the wait the task is answered as it is.
            let cancelled = async {
                let summary = served.backend.cancel(served.route, id).await?;
                view::wait(served, (&summary, false), ended).await
            };
            if let Ok(cancelled) = tokio::time::timeout(CANCEL_WAIT, cancelled).await {
                cancelled.map_err(refused)?;
            }
            view::task(served, id, detail).await.map_err(refused)
        }
        "ListTasks" => super::list::list(served, params).await,
        method if method.contains("PushNotificationConfig") => Err(RpcError::push_not_supported()),
        "GetExtendedAgentCard" => Err(RpcError::no_extended_card()),
        _ => Err(RpcError::method_not_found()),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{configuration as configuration_of, inbound};

    fn params(field: &str, value: &Value) -> Value {
        let mut message =
            json!({"messageId": "m-1", "role": "ROLE_USER", "parts": [{"text": "hi"}]});
        if let Some(object) = message.as_object_mut() {
            object.insert(field.to_owned(), value.clone());
        }
        json!({"message": message})
    }

    #[test]
    fn a_message_continues_the_task_it_names() {
        let task =
            |field: &str, value: Value| inbound(&params(field, &value)).map(|read| read.task);
        assert_eq!(task("taskId", json!("t-1")), Ok(Some("t-1".to_owned())));
        assert_eq!(task("contextId", json!("t-1")), Ok(Some("t-1".to_owned())));
        assert_eq!(task("taskId", json!("")), Ok(None));
        assert_eq!(task("taskId", Value::Null), Ok(None));
    }

    #[test]
    fn a_send_configuration_is_read_whole_before_anything_is_delivered() {
        let read = |configuration: Value| {
            configuration_of(&json!({"configuration": configuration}))
                .map(|(immediately, detail)| (immediately, detail.history))
        };
        assert_eq!(read(Value::Null), Ok((false, None)));
        assert_eq!(read(json!({})), Ok((false, None)));
        assert_eq!(read(json!({"returnImmediately": false})), Ok((false, None)));
        let both = json!({"returnImmediately": true, "historyLength": 2});
        assert_eq!(read(both), Ok((true, Some(2))));
        for malformed in [
            json!([]),
            json!("{}"),
            json!({"returnImmediately": "true"}),
            json!({"returnImmediately": 1}),
            json!({"historyLength": -1}),
        ] {
            let read = read(malformed.clone());
            assert!(
                read.is_err_and(|error| error.code == -32_602),
                "{malformed}"
            );
        }
    }

    #[test]
    fn a_part_without_content_is_an_invalid_parameter() {
        let code = |part: Value| {
            let params = json!({"message": {
                "messageId": "m-1", "role": "ROLE_USER", "parts": [part],
            }});
            inbound(&params).err().map(|error| error.code)
        };
        assert_eq!(code(json!({"raw": "aGk="})), Some(-32_005));
        assert_eq!(
            code(json!({"url": "https://example.invalid/a"})),
            Some(-32_005)
        );
        assert_eq!(code(json!({"raw": null})), Some(-32_602));
        assert_eq!(code(json!({"url": null})), Some(-32_602));
    }

    #[test]
    fn a_task_id_that_is_not_a_string_is_refused() {
        for field in ["taskId", "contextId"] {
            let read = inbound(&params(field, &json!(7))).map(|read| read.task);
            assert!(read.is_err_and(|error| error.code == -32_602), "{field}");
        }
    }
}
