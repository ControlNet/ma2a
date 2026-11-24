//! `SendStreamingMessage` and `SubscribeToTask`: one task's lifecycle as Server-Sent Events.
//!
//! A stream begins with the task, then carries a status or artifact update for each new
//! event, and closes once the task ended (or, after a message, once it waits for input).

use std::{convert::Infallible, pin::Pin};

use axum::response::{
    IntoResponse as _, Response,
    sse::{Event, KeepAlive, Sse},
};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use super::{
    backend::{Backend, Refusal, Route, ended},
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
    /// Most history messages the first frame's task carries.
    pub(super) history: Option<usize>,
    /// Close once the task waits for input, not only once it ended.
    pub(super) until_waiting: bool,
    /// For a message to a waiting task: follow from the sequence number it was sent at,
    /// and close only on a state the task reaches after it.
    pub(super) sent_at: Option<u32>,
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

/// Where a followed stream stands.
struct Position {
    /// The state the stream last showed.
    current: String,
    /// The states the stream closes on.
    finishes: fn(&str) -> bool,
    /// The newest event the first frame already showed.
    shown: u64,
}

impl Position {
    /// Whether the stream closes with its first frame. An ended task never changes again;
    /// after a message to a waiting task, only a state reached since then counts.
    fn closes_at_once(&self, sent_at: Option<u32>) -> bool {
        ended(&self.current) || (sent_at.is_none() && (self.finishes)(&self.current))
    }

    /// The update one followed event becomes, and whether the stream closes with it. An
    /// event the first frame already showed still moves the state, but is not sent again.
    fn forward(&mut self, event: &Value, (task, base): (&str, &str)) -> (Option<Value>, bool) {
        let state = text(event, "/kind") == "state";
        if state {
            text(event, "/state").clone_into(&mut self.current);
        }
        let seq = event.get("seq").and_then(Value::as_u64).unwrap_or_default();
        let update = if seq > self.shown {
            update(event, (task, &self.current, base))
        } else {
            None
        };
        (update, state && (self.finishes)(&self.current))
    }
}

/// Starts the stream: the task as it is now, then its updates.
pub(super) fn respond(owned: Owned, lifecycle: Lifecycle) -> Response {
    let (frames, received) = mpsc::channel::<Result<Event, Infallible>>(BUFFERED);
    tokio::spawn(async move {
        while_connected(&frames, Box::pin(produce(&owned, &lifecycle, &frames))).await;
    });
    Sse::new(ReceiverStream::new(received))
        .keep_alive(KeepAlive::default())
        .into_response()
}

/// Runs `work` until it ends or the client disconnects, whichever comes first: a target
/// that never answers the first read must not keep a disconnected client's stream alive.
async fn while_connected<T>(
    frames: &mpsc::Sender<Result<Event, Infallible>>,
    work: impl Future<Output = T>,
) {
    tokio::select! {
        biased;
        () = frames.closed() => {}
        _ended = work => {}
    }
}

/// Sends one JSON-RPC response as an event; `false` once the client is gone.
async fn send(frames: &mpsc::Sender<Result<Event, Infallible>>, response: &Value) -> bool {
    frames
        .send(Ok(Event::default().data(response.to_string())))
        .await
        .is_ok()
}

/// What a followed stream does next.
enum Next {
    Event(Value),
    Followed(Result<(), Refusal>),
    /// The client disconnected.
    Gone,
}

/// Waits for the next event, the end of the follow, or the client's disconnect, whichever
/// comes first: a quiet task must not keep a disconnected client's follow alive.
async fn next<F: Future<Output = Result<(), Refusal>>>(
    received: &mut mpsc::UnboundedReceiver<Value>,
    following: Pin<&mut F>,
    frames: &mpsc::Sender<Result<Event, Infallible>>,
) -> Next {
    tokio::select! {
        Some(event) = received.recv() => Next::Event(event),
        outcome = following => Next::Followed(outcome),
        () = frames.closed() => Next::Gone,
    }
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
        history: lifecycle.history,
        artifacts: true,
    };
    // The first frame, the state, and the cursor all come from one snapshot, so the stream
    // neither repeats an event the frame shows nor closes on a state it does not.
    let first = match view::of(served, &details, detail).await {
        Ok(first) => first,
        Err(refusal) => return send(frames, &refused(refusal)).await.then_some(()),
    };
    let shown = view::last_seq(&details);
    let mut position = Position {
        current: text(&details, "/task/state").to_owned(),
        finishes: if lifecycle.until_waiting {
            settled
        } else {
            ended
        },
        shown: u64::from(shown),
    };
    let settled_now = position.closes_at_once(lifecycle.sent_at);
    if !send(frames, &success(id, &json!({"task": first}))).await || settled_now {
        return Some(());
    }
    let after = lifecycle.sent_at.unwrap_or(shown);
    let (events, mut received) = mpsc::unbounded_channel();
    let following = owned.backend.follow((&owned.route, (task, after)), &events);
    tokio::pin!(following);
    let mut forward = |event: Value| position.forward(&event, (task, &owned.base));
    let followed = loop {
        let event = match next(&mut received, following.as_mut(), frames).await {
            Next::Event(event) => event,
            Next::Followed(outcome) => break outcome,
            // Ending here drops the follow, which ends a remote watch with it.
            Next::Gone => return None,
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

#[cfg(test)]
#[path = "stream_tests.rs"]
mod tests;
