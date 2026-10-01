//! Whole A2A tasks: the task's details, its history, and waiting for it to settle.

use serde_json::{Value, json};
use tokio::sync::mpsc;

use super::{
    backend::{Backend, Refusal, Route, ended, fields},
    mapping::{self, Detail},
};

/// One route's Agent as the adapter serves it: where it lives and its public base URL.
#[derive(Clone, Copy, Debug)]
pub(super) struct Served<'a> {
    pub(super) backend: &'a Backend,
    pub(super) route: &'a Route,
    pub(super) base: &'a str,
}

fn text<'a>(value: &'a Value, pointer: &str) -> &'a str {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// Returns whether a task in this local API state waits for nothing from the Agent: it
/// ended, or it waits for its creator's input.
pub(super) fn settled(state: &str) -> bool {
    state == "waiting" || ended(state)
}

/// The sequence number of the newest event of a task summary or details.
pub(super) fn last_seq(task: &Value) -> u32 {
    let summary = task.get("task").unwrap_or(task);
    summary
        .get("last_seq")
        .and_then(Value::as_u64)
        .and_then(|seq| u32::try_from(seq).ok())
        .unwrap_or(0)
}

/// The messages of a task, oldest first.
///
/// A local task's history is every retained message event; another Endpoint's shows only
/// what its details carry, the input and the result.
async fn history(served: Served<'_>, details: &Value) -> Result<Vec<Value>, Refusal> {
    let task = text(details, "/task/task_id");
    let Route::Local(_) = served.route else {
        let mut history = Vec::new();
        for (field, id) in [("input", "input"), ("result", "result")] {
            if let Some(message) = details.get(field).filter(|message| !message.is_null()) {
                history.push(mapping::message(message, (task, id)));
            }
        }
        return Ok(history);
    };
    // The input opens the task; the event log records the messages after it.
    let mut history: Vec<Value> = details
        .get("input")
        .filter(|input| !input.is_null())
        .map(|input| mapping::message(input, (task, "input")))
        .into_iter()
        .collect();
    let (mut after, mut more) = (0_u64, true);
    while more {
        let fields = fields([("task_id", json!(task)), ("after", json!(after))]);
        let page = served.backend.local("agent_task_events", fields).await?;
        for event in page
            .get("events")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let seq = event.get("seq").and_then(Value::as_u64).unwrap_or(after);
            after = seq;
            if let Some(message) = event.get("message").filter(|message| !message.is_null()) {
                history.push(mapping::message(message, (task, &format!("m{seq}"))));
            }
        }
        more = page.get("more").and_then(Value::as_bool).unwrap_or(false);
    }
    Ok(history)
}

/// One task as an A2A `Task`.
pub(super) async fn task(served: Served<'_>, id: &str, detail: Detail) -> Result<Value, Refusal> {
    let details = served.backend.details(served.route, id).await?;
    let history = history(served, &details).await?;
    Ok(mapping::task(&details, (&history, served.base), detail))
}

/// Waits until a task whose summary is `summary` reaches a state `until` accepts.
///
/// With `fresh`, only a state the task reaches after `summary` counts: a message just sent
/// to a waiting task leaves its summary waiting until the Agent takes the message.
pub(super) async fn wait(
    served: Served<'_>,
    (summary, fresh): (&Value, bool),
    until: fn(&str) -> bool,
) -> Result<(), Refusal> {
    let summary = summary.get("task").unwrap_or(summary);
    if !fresh && until(text(summary, "/state")) {
        return Ok(());
    }
    let task = text(summary, "/task_id");
    let (events, mut received) = mpsc::unbounded_channel();
    let following = served
        .backend
        .follow((served.route, (task, last_seq(summary))), &events);
    tokio::pin!(following);
    let reached = |event: &Value| text(event, "/kind") == "state" && until(text(event, "/state"));
    loop {
        tokio::select! {
            Some(event) = received.recv() => if reached(&event) {
                return Ok(());
            },
            followed = &mut following => {
                followed?;
                // The task ended: whatever is still queued is its last events.
                return Ok(());
            }
        }
    }
}
