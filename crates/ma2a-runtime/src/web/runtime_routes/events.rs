use std::{convert::Infallible, sync::Arc, time::Duration};

use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse as _, Response, Sse, sse::Event, sse::KeepAlive},
};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::{OwnedSemaphorePermit, mpsc};
use tokio_stream::wrappers::ReceiverStream;

use super::{SnapshotStamp, WebState, cookie};

const EVENT_QUEUE_CAPACITY: usize = 8;
pub(super) const EVENT_CHANNEL_CAPACITY: usize = EVENT_QUEUE_CAPACITY + 1;
pub(super) const POLL_INTERVAL: Duration = Duration::from_secs(1);
pub(super) const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);

struct EventProducer {
    state: WebState,
    bearer: String,
    cursor: EventCursor,
    sender: mpsc::Sender<Result<Event, Infallible>>,
    _permit: OwnedSemaphorePermit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SnapshotChange {
    Unchanged,
    Consecutive,
    Resync,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct EventCursor {
    pub(super) revision: u64,
    pub(super) boot_id: String,
}

#[derive(Deserialize)]
pub(crate) struct EventsQuery {
    since: u64,
}

pub(crate) async fn events(
    State(state): State<WebState>,
    headers: HeaderMap,
    Query(query): Query<EventsQuery>,
) -> Response {
    let Some(bearer) = cookie(&headers).map(str::to_owned) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if state.auth.validate(&bearer).await.is_err() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Ok(permit) = Arc::clone(&state.event_connections).try_acquire_owned() else {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    };
    let Ok(baseline) = fetch_stamp(&state).await else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let (sender, receiver) = mpsc::channel(EVENT_CHANNEL_CAPACITY);
    if baseline.revision == query.since {
        tokio::spawn(
            EventProducer {
                state,
                bearer,
                cursor: EventCursor {
                    revision: baseline.revision,
                    boot_id: baseline.boot_id,
                },
                sender,
                _permit: permit,
            }
            .run(),
        );
    } else {
        let _result = sender.try_send(Ok(resync_event(baseline.revision)));
    }
    Sse::new(ReceiverStream::new(receiver))
        .keep_alive(KeepAlive::new().interval(HEARTBEAT_INTERVAL))
        .into_response()
}

impl EventProducer {
    async fn run(mut self) {
        let mut interval = tokio::time::interval(POLL_INTERVAL);
        loop {
            if !wait_for_poll(&self.sender, &mut interval).await {
                return;
            }
            if self.state.auth.validate(&self.bearer).await.is_err() {
                return;
            }
            let Ok(current) = fetch_stamp(&self.state).await else {
                let _result = self.sender.try_send(Ok(resync_event(self.cursor.revision)));
                return;
            };
            if !self.cursor.emit(&self.sender, &current) {
                return;
            }
        }
    }
}

impl EventCursor {
    pub(super) fn classify(&self, current: &SnapshotStamp) -> SnapshotChange {
        if current.boot_id != self.boot_id {
            SnapshotChange::Resync
        } else if current.revision == self.revision {
            SnapshotChange::Unchanged
        } else if self.revision.checked_add(1) == Some(current.revision) {
            SnapshotChange::Consecutive
        } else {
            SnapshotChange::Resync
        }
    }

    pub(super) fn emit(
        &mut self,
        sender: &mpsc::Sender<Result<Event, Infallible>>,
        current: &SnapshotStamp,
    ) -> bool {
        match self.classify(current) {
            SnapshotChange::Unchanged => return true,
            SnapshotChange::Resync => {
                let _result = sender.try_send(Ok(resync_event(current.revision)));
                return false;
            }
            SnapshotChange::Consecutive => {}
        }
        if sender.capacity() <= 1 {
            let _result = sender.try_send(Ok(resync_event(current.revision)));
            return false;
        }
        let payload =
            json!({"type": "snapshot_invalidated", "revision": current.revision, "changed": {}});
        if sender
            .try_send(Ok(Event::default()
                .id(current.revision.to_string())
                .data(payload.to_string())))
            .is_err()
        {
            return false;
        }
        self.revision = current.revision;
        true
    }
}

pub(super) async fn wait_for_poll(
    sender: &mpsc::Sender<Result<Event, Infallible>>,
    interval: &mut tokio::time::Interval,
) -> bool {
    tokio::select! {
        biased;
        () = sender.closed() => false,
        _ = interval.tick() => true,
    }
}

fn resync_event(revision: u64) -> Event {
    Event::default()
        .event("resync-required")
        .data(json!({"revision": revision}).to_string())
}

async fn fetch_stamp(state: &WebState) -> Result<SnapshotStamp, StatusCode> {
    let runtime = state
        .runtime
        .as_ref()
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let response = runtime
        .call(&crate::api::Command::snapshot_stamp())
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let value: Value =
        serde_json::from_slice(&response).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let payload = value
        .pointer("/result/payload")
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(SnapshotStamp {
        revision: payload
            .get("revision")
            .and_then(Value::as_u64)
            .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?,
        boot_id: payload
            .get("runtime_boot_id")
            .and_then(Value::as_str)
            .filter(|id| id.len() == 32)
            .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?
            .to_owned(),
    })
}
