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
    // Only the events the details counted, so the history agrees with the state beside it.
    let last = u64::from(last_seq(details));
    let mut after = Some(0);
    while let Some(from) = after.filter(|from| *from < last) {
        let fields = fields([("task_id", json!(task)), ("after", json!(from))]);
        let page = served.backend.local("agent_task_events", fields).await?;
        after = add_page(&mut history, &page, (task, from, last));
    }
    Ok(history)
}

/// Adds the messages of one page of a task's events after `after`, up to the event `last`, to
/// `history`; returns where the next page starts, while one is left to read.
fn add_page(
    history: &mut Vec<Value>,
    page: &Value,
    (task, after, last): (&str, u64, u64),
) -> Option<u64> {
    let mut after = after;
    for event in page
        .get("events")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let seq = event.get("seq").and_then(Value::as_u64).unwrap_or(after);
        if seq > last {
            return None;
        }
        after = seq;
        if let Some(message) = event.get("message").filter(|message| !message.is_null()) {
            history.push(mapping::message(message, (task, &format!("m{seq}"))));
        }
    }
    page.get("more")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        .then_some(after)
}

/// One task as an A2A `Task`.
pub(super) async fn task(served: Served<'_>, id: &str, detail: Detail) -> Result<Value, Refusal> {
    let details = served.backend.details(served.route, id).await?;
    of(served, &details, detail).await
}

/// The task one snapshot of its details shows, as an A2A `Task`.
pub(super) async fn of(
    served: Served<'_>,
    details: &Value,
    detail: Detail,
) -> Result<Value, Refusal> {
    // A waiting task's status shows its newest Agent message, found in its history, so only
    // a view of another task that keeps no history reads none.
    let history = if detail.history == Some(0) && text(details, "/task/state") != "waiting" {
        Vec::new()
    } else {
        history(served, details).await?
    };
    Ok(mapping::task(details, (&history, served.base), detail))
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use ma2a_core::service::AgentId;
    use ma2a_runtime::ipc::IpcPaths;

    use super::{Served, add_page, of};
    use crate::a2a::{
        backend::{Backend, Route},
        mapping::Detail,
    };

    #[test]
    fn history_stops_at_the_last_event_the_details_counted() {
        // Given: a page holding a message the details did not count yet.
        let message = json!({"role": "agent", "parts": [{"text": "hi"}]});
        let page = json!({"events": [
            {"seq": 1, "message": message},
            {"seq": 2, "message": null},
            {"seq": 3, "message": message},
        ], "more": true});

        // When
        let mut history = Vec::new();
        let next = add_page(&mut history, &page, ("t", 0, 2));

        // Then
        assert_eq!(history.len(), 1);
        assert_eq!(next, None);
        let mut whole = Vec::new();
        assert_eq!(add_page(&mut whole, &page, ("t", 0, 3)), Some(3));
        assert_eq!(whole.len(), 2);
    }

    #[tokio::test]
    async fn a_view_without_history_reads_no_events() -> Result<(), Box<dyn std::error::Error>> {
        // Given: a Runtime that is not running, so reading any event fails.
        let state_dir = std::env::temp_dir().join("ma2a-a2a-view-no-runtime");
        let backend = Backend {
            paths: IpcPaths::new(&state_dir)?,
            state_dir,
        };
        let route = Route::Local(AgentId::new("lead")?);
        let served = Served {
            backend: &backend,
            route: &route,
            base: "http://127.0.0.1",
        };
        let details =
            |state: &str| json!({"task": {"task_id": "t", "state": state, "last_seq": 4}});
        let none = Detail {
            history: Some(0),
            artifacts: true,
        };

        // Then: an ended task's view with no history needs no event.
        assert!(of(served, &details("completed"), none).await.is_ok());
        // A waiting task's status still shows the newest Agent message, read from its events.
        assert!(of(served, &details("waiting"), none).await.is_err());
        Ok(())
    }
}
