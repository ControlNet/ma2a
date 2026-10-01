//! `SendStreamingMessage` and `SubscribeToTask`: one task's lifecycle as Server-Sent Events.
//!
//! A stream begins with the task, then carries a status or artifact update for each new
//! event, and closes once the task ended (or, after a message, once it waits for input).

use std::convert::Infallible;

use axum::response::{
    IntoResponse as _, Response,
    sse::{Event, KeepAlive, Sse},
};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use super::{
    backend::{Backend, Route, ended},
    mapping::{self, Detail},
    rpc::{RpcError, success},
    view::{self, Served, settled},
};

/// Frames buffered for a slow client before the producer waits.
const BUFFERED: usize = 16;

/// What the stream answers and when it closes.
pub(super) struct Lifecycle {
    pub(super) id: Value,
    pub(super) method: &'static str,
    pub(super) task: String,
    /// Close once the task waits for input, not only once it ended.
    pub(super) until_waiting: bool,
}

/// Everything a stream producer owns.
pub(super) struct Owned {
    pub(super) backend: Backend,
    pub(super) route: Route,
    pub(super) base: String,
}

fn text<'a>(value: &'a Value, pointer: &str) -> &'a str {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// The status or artifact update one local API event becomes, if any.
fn update(event: &Value, (task, current, base): (&str, &str, &str)) -> Option<Value> {
    let at_ms = event
        .get("at_ms")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let status = |message: Option<Value>| {
        let mut status =
            json!({"state": mapping::state(current), "timestamp": super::time::format(at_ms)});
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

/// Starts the stream: the task as it is now, then its updates.
pub(super) fn respond(owned: Owned, lifecycle: Lifecycle) -> Response {
    let (frames, received) = mpsc::channel::<Result<Event, Infallible>>(BUFFERED);
    tokio::spawn(async move {
        let _ended = produce(&owned, &lifecycle, &frames).await;
    });
    Sse::new(ReceiverStream::new(received))
        .keep_alive(KeepAlive::default())
        .into_response()
}

/// Sends one JSON-RPC response as an event; `false` once the client is gone.
async fn send(frames: &mpsc::Sender<Result<Event, Infallible>>, response: &Value) -> bool {
    frames
        .send(Ok(Event::default().data(response.to_string())))
        .await
        .is_ok()
}

async fn produce(
    owned: &Owned,
    lifecycle: &Lifecycle,
    frames: &mpsc::Sender<Result<Event, Infallible>>,
) -> Option<()> {
    let served = Served {
        backend: &owned.backend,
        route: &owned.route,
        base: &owned.base,
    };
    let (id, task) = (&lifecycle.id, lifecycle.task.as_str());
    let refused = |refusal| RpcError::refused(lifecycle.method, &refusal).response(id);
    let details = match owned.backend.details(&owned.route, task).await {
        Ok(details) => details,
        Err(refusal) => return send(frames, &refused(refusal)).await.then_some(()),
    };
    let detail = Detail {
        history: None,
        artifacts: true,
    };
    let first = match view::task(served, task, detail).await {
        Ok(first) => first,
        Err(refusal) => return send(frames, &refused(refusal)).await.then_some(()),
    };
    let finishes = |state: &str| {
        if lifecycle.until_waiting {
            settled(state)
        } else {
            ended(state)
        }
    };
    let mut current = text(&details, "/task/state").to_owned();
    if !send(frames, &success(id, &json!({"task": first}))).await || finishes(&current) {
        return Some(());
    }
    let (events, mut received) = mpsc::unbounded_channel();
    let following = owned
        .backend
        .follow((&owned.route, (task, view::last_seq(&details))), &events);
    tokio::pin!(following);
    let mut forward = |event: Value| {
        if text(&event, "/kind") == "state" {
            text(&event, "/state").clone_into(&mut current);
        }
        let last = text(&event, "/kind") == "state" && finishes(&current);
        (update(&event, (task, &current, &owned.base)), last)
    };
    let followed = loop {
        let event = tokio::select! {
            Some(event) = received.recv() => event,
            outcome = &mut following => break outcome,
        };
        let (update, last) = forward(event);
        if let Some(update) = update {
            send(frames, &success(id, &update)).await.then_some(())?;
        }
        if last {
            return Some(());
        }
    };
    // The task ended: whatever is still queued is its last events.
    while let Ok(event) = received.try_recv() {
        let (update, last) = forward(event);
        if let Some(update) = update {
            send(frames, &success(id, &update)).await.then_some(())?;
        }
        if last {
            return Some(());
        }
    }
    if let Err(refusal) = followed {
        send(frames, &refused(refusal)).await;
    }
    Some(())
}
