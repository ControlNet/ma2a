//! Local API task views as A2A v1.0 objects.
//!
//! The A2A context of a task is the task itself: `contextId` always equals `id`.

use serde_json::{Map, Value, json};

/// Maps one local API task state onto its A2A `TaskState`.
pub(super) fn state(name: &str) -> &'static str {
    match name {
        "queued" => "TASK_STATE_SUBMITTED",
        "running" => "TASK_STATE_WORKING",
        "waiting" => "TASK_STATE_INPUT_REQUIRED",
        "completed" => "TASK_STATE_COMPLETED",
        "failed" => "TASK_STATE_FAILED",
        "cancelled" => "TASK_STATE_CANCELED",
        _ => "TASK_STATE_UNSPECIFIED",
    }
}

fn text<'a>(value: &'a Value, pointer: &str) -> Option<&'a str> {
    value.pointer(pointer).and_then(Value::as_str)
}

/// One A2A part from a local API part: data parts carry their JSON value.
fn part(local: &Value) -> Value {
    let content = text(local, "/content").unwrap_or_default();
    if text(local, "/kind") == Some("data")
        && let Ok(data) = serde_json::from_str::<Value>(content)
    {
        return json!({"data": data, "mediaType": "application/json"});
    }
    json!({"text": content, "mediaType": "text/plain"})
}

/// One A2A message of `task` from a local API message; `id` names it within the task.
pub(super) fn message(local: &Value, (task, id): (&str, &str)) -> Value {
    let role = if text(local, "/role") == Some("agent") {
        "ROLE_AGENT"
    } else {
        "ROLE_USER"
    };
    let parts: Vec<Value> = local
        .get("parts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(part)
        .collect();
    json!({
        "messageId": format!("{task}-{id}"),
        "contextId": task,
        "taskId": task,
        "role": role,
        "parts": parts,
    })
}

/// An agent message made of one text part.
pub(super) fn agent_text(task: &str, (id, content): (&str, &str)) -> Value {
    let local = json!({"role": "agent", "parts": [{"kind": "text", "content": content}]});
    message(&local, (task, id))
}

/// Percent-encodes one URL path segment.
pub(super) fn segment(raw: &str) -> String {
    raw.bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

/// One A2A artifact from a local API artifact; its content is served at `base`'s
/// artifact URL.
pub(super) fn artifact(local: &Value, (base, task): (&str, &str)) -> Value {
    let name = text(local, "/name").unwrap_or_default();
    json!({
        "artifactId": name,
        "name": name,
        "parts": [{
            "url": format!("{base}/tasks/{task}/artifacts/{}", segment(name)),
            "filename": name,
            "mediaType": text(local, "/media_type").unwrap_or("application/octet-stream"),
        }],
        "metadata": {
            "size": local.get("size").cloned().unwrap_or(Value::Null),
            "blake3": local.get("blake3").cloned().unwrap_or(Value::Null),
        },
    })
}

/// What one task view includes besides its status.
#[derive(Clone, Copy, Debug)]
pub(super) struct Detail {
    /// Most messages of history, newest last; `None` is no limit.
    pub(super) history: Option<usize>,
    pub(super) artifacts: bool,
}

/// The status of a task: its state, when it last changed, and the message that explains
/// it (the result, the failure, or the newest Agent message while it waits).
pub(super) fn status(details: &Value, latest: Option<&Value>) -> Value {
    let summary = details.get("task").unwrap_or(details);
    let task = text(summary, "/task_id").unwrap_or_default();
    let local_state = text(summary, "/state").unwrap_or_default();
    let at_ms = ["/ended_at_ms", "/started_at_ms", "/created_at_ms"]
        .iter()
        .find_map(|pointer| summary.pointer(pointer).and_then(Value::as_u64))
        .unwrap_or_default();
    let explanation = match local_state {
        "completed" => details
            .get("result")
            .filter(|value| !value.is_null())
            .map(|result| message(result, (task, "result"))),
        "failed" => text(details, "/error").map(|error| agent_text(task, ("error", error))),
        "waiting" => latest.cloned(),
        _ => None,
    };
    let mut status = json!({"state": state(local_state), "timestamp": super::time::format(at_ms)});
    if let (Some(explanation), Some(object)) = (explanation, status.as_object_mut()) {
        object.insert("message".to_owned(), explanation);
    }
    status
}

/// One A2A task from a local API task (summary or details) and its message history.
pub(super) fn task(details: &Value, (history, base): (&[Value], &str), detail: Detail) -> Value {
    let summary = details.get("task").unwrap_or(details);
    let id = text(summary, "/task_id").unwrap_or_default();
    let latest_agent = history
        .iter()
        .rev()
        .find(|message| text(message, "/role") == Some("ROLE_AGENT"));
    let mut task = Map::new();
    task.insert("id".to_owned(), json!(id));
    task.insert("contextId".to_owned(), json!(id));
    task.insert("status".to_owned(), status(details, latest_agent));
    if detail.artifacts {
        let artifacts: Vec<Value> = details
            .get("artifacts")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|local| artifact(local, (base, id)))
            .collect();
        task.insert("artifacts".to_owned(), json!(artifacts));
    }
    let keep = detail.history.unwrap_or(usize::MAX).min(history.len());
    let recent = history.get(history.len() - keep..).unwrap_or_default();
    task.insert("history".to_owned(), json!(recent));
    task.insert(
        "metadata".to_owned(),
        json!({"ma2a": {
            "agent": summary.get("agent"),
            "creatorEndpointId": summary.get("creator_endpoint_id"),
            "fromAgent": summary.get("from_agent"),
            "reason": summary.get("reason"),
            "progress": details.get("progress"),
        }}),
    );
    Value::Object(task)
}

#[cfg(test)]
mod tests {
    use super::{segment, state};

    #[test]
    fn every_local_state_has_its_a2a_state() {
        let mapped: Vec<&str> = [
            "queued",
            "running",
            "waiting",
            "completed",
            "failed",
            "cancelled",
        ]
        .into_iter()
        .map(state)
        .collect();
        assert_eq!(
            mapped,
            [
                "TASK_STATE_SUBMITTED",
                "TASK_STATE_WORKING",
                "TASK_STATE_INPUT_REQUIRED",
                "TASK_STATE_COMPLETED",
                "TASK_STATE_FAILED",
                "TASK_STATE_CANCELED",
            ]
        );
    }

    #[test]
    fn path_segments_are_percent_encoded() {
        assert_eq!(segment("report v2/ä.txt"), "report%20v2%2F%C3%A4.txt");
    }
}
