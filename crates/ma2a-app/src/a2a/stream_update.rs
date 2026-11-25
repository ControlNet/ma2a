//! Turning followed Agent v1 events into A2A `statusUpdate` and `artifactUpdate` frames,
//! tracking the task's current state and last state-change timestamp.

use serde_json::{Value, json};

use super::super::{backend::ended, mapping, time};

pub(super) fn text<'a>(value: &'a Value, pointer: &str) -> &'a str {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// The status or artifact update one local API event becomes, if any.
fn update(
    event: &Value,
    (task, current, changed_ms, base): (&str, &str, u64, &str),
) -> Option<Value> {
    let status = |message: Option<Value>| {
        let mut status = json!({
            "state": mapping::state(current),
            "timestamp": time::format(changed_ms),
        });
        if let (Some(message), Some(object)) = (message, status.as_object_mut()) {
            object.insert("message".to_owned(), message);
        }
        json!({"statusUpdate": {"taskId": task, "contextId": task, "status": status}})
    };
    let seq = event.get("seq").and_then(Value::as_u64).unwrap_or_default();
    match text(event, "/kind") {
        "state" => Some(status(None)),
        // The creator's own messages are already in the task's history.
        "message" if text(event, "/message/role") == "agent" => {
            let message = event.get("message").unwrap_or(&Value::Null);
            Some(status(Some(mapping::message(
                message,
                (task, &format!("m{seq}")),
            ))))
        }
        "progress" => {
            let progress = text(event, "/text");
            Some(status(Some(mapping::agent_text(
                task,
                (&format!("p{seq}"), progress),
            ))))
        }
        "artifact" => {
            let artifact =
                mapping::artifact(event.get("artifact").unwrap_or(&Value::Null), (base, task));
            Some(json!({"artifactUpdate": {
                "taskId": task,
                "contextId": task,
                "artifact": artifact,
                "append": false,
                "lastChunk": true,
            }}))
        }
        _ => None,
    }
}

/// Where a followed stream stands.
pub(super) struct Position {
    /// The state the stream last showed.
    pub(super) current: String,
    /// When the task's state last changed, in Unix milliseconds.
    changed_ms: u64,
    /// The states the stream closes on.
    finishes: fn(&str) -> bool,
    /// The newest event the first frame already showed.
    shown: u64,
    /// Whether the first frame showed a state the stream closes on.
    shown_finished: bool,
}

impl Position {
    /// Where a stream stands once its first frame showed `current` as of event `shown`.
    pub(super) fn new(
        (current, changed_ms): (String, u64),
        finishes: fn(&str) -> bool,
        shown: u64,
    ) -> Self {
        Self {
            shown_finished: finishes(&current),
            current,
            changed_ms,
            finishes,
            shown,
        }
    }

    /// Whether the stream closes with its first frame. An ended task never changes again;
    /// after a message to a waiting task, only a state reached since then counts.
    pub(super) fn closes_at_once(&self, sent_at: Option<u32>) -> bool {
        ended(&self.current) || (sent_at.is_none() && (self.finishes)(&self.current))
    }

    /// The update one followed event becomes, and whether the stream closes with it. An
    /// event the first frame already showed still moves the state, but is not sent again,
    /// and closes the stream only when the frame showed a state it closes on: a wait the task
    /// left again before the frame must not end a stream whose frame shows it working.
    pub(super) fn forward(
        &mut self,
        event: &Value,
        (task, base): (&str, &str),
    ) -> (Option<Value>, bool) {
        let state = text(event, "/kind") == "state";
        if state {
            text(event, "/state").clone_into(&mut self.current);
            self.changed_ms = event
                .get("at_ms")
                .and_then(Value::as_u64)
                .unwrap_or(self.changed_ms);
        }
        let seq = event.get("seq").and_then(Value::as_u64).unwrap_or_default();
        let update = if seq > self.shown {
            update(event, (task, &self.current, self.changed_ms, base))
        } else {
            None
        };
        let closes = (self.finishes)(&self.current) && (seq > self.shown || self.shown_finished);
        (update, state && closes)
    }
}
