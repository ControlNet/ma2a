use std::{convert::Infallible, sync::Arc, time::Duration};

use axum::{
    Json,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse as _, Response, Sse, sse::Event, sse::KeepAlive},
};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::{OwnedSemaphorePermit, mpsc};
use tokio_stream::wrappers::ReceiverStream;

use super::{WebState, cookie};

const EVENT_QUEUE_CAPACITY: usize = 8;
const POLL_INTERVAL: Duration = Duration::from_secs(1);
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);

struct EventProducer {
    state: WebState,
    bearer: String,
    revision: u64,
    sender: mpsc::Sender<Result<Event, Infallible>>,
    _permit: OwnedSemaphorePermit,
}

#[derive(Deserialize)]
pub(super) struct EventsQuery {
    since: u64,
}

pub(super) async fn snapshot(State(state): State<WebState>, headers: HeaderMap) -> Response {
    let Some(bearer) = cookie(&headers) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if state.auth.authenticate(bearer).await.is_err() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    match fetch_snapshot(&state).await {
        Ok((_, payload)) => Json(payload).into_response(),
        Err(status) => status.into_response(),
    }
}

pub(super) async fn events(
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
    let Ok((baseline, _)) = fetch_snapshot(&state).await else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let (sender, receiver) = mpsc::channel(EVENT_QUEUE_CAPACITY);
    if baseline == query.since {
        tokio::spawn(
            EventProducer {
                state,
                bearer,
                revision: baseline,
                sender,
                _permit: permit,
            }
            .run(),
        );
    } else {
        let _result = sender.try_send(Ok(resync_event(baseline)));
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
                let _result = self.sender.try_send(Ok(resync_event(self.revision)));
                return;
            }
            let Ok((current, _)) = fetch_snapshot(&self.state).await else {
                let _result = self.sender.try_send(Ok(resync_event(self.revision)));
                return;
            };
            if current == self.revision {
                continue;
            }
            if self.revision.checked_add(1) != Some(current) {
                let _result = self.sender.try_send(Ok(resync_event(current)));
                return;
            }
            let payload =
                json!({"type": "snapshot_invalidated", "revision": current, "changed": {}});
            if self
                .sender
                .try_send(Ok(Event::default()
                    .id(current.to_string())
                    .data(payload.to_string())))
                .is_err()
            {
                return;
            }
            self.revision = current;
        }
    }
}

async fn wait_for_poll(
    sender: &mpsc::Sender<Result<Event, Infallible>>,
    interval: &mut tokio::time::Interval,
) -> bool {
    tokio::select! {
        biased;
        () = sender.closed() => false,
        _ = interval.tick() => true,
    }
}

async fn fetch_snapshot(state: &WebState) -> Result<(u64, Value), StatusCode> {
    let runtime = state
        .runtime
        .as_ref()
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let response = runtime
        .call(&crate::api::Command::snapshot_fetch())
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let value: Value =
        serde_json::from_slice(&response).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let revision = value
        .get("revision")
        .and_then(Value::as_u64)
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
    let payload = value
        .pointer("/result/payload")
        .cloned()
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok((revision, payload))
}

fn resync_event(revision: u64) -> Event {
    Event::default()
        .event("resync-required")
        .data(json!({"revision": revision}).to_string())
}

#[cfg(test)]
mod tests {
    use axum::response::{IntoResponse as _, Sse};
    use tokio_stream::{StreamExt as _, wrappers::ReceiverStream};

    use super::*;

    #[tokio::test(start_paused = true)]
    async fn heartbeat_is_an_unrevisioned_sse_comment() {
        // Given
        let (_sender, receiver) = mpsc::channel::<Result<Event, Infallible>>(1);
        let response = Sse::new(ReceiverStream::new(receiver))
            .keep_alive(KeepAlive::new().interval(HEARTBEAT_INTERVAL))
            .into_response();
        let mut body = response.into_body().into_data_stream();

        // When
        tokio::time::advance(HEARTBEAT_INTERVAL).await;
        let frame = body.next().await;

        // Then
        assert_eq!(
            frame.transpose().expect("heartbeat body error"),
            Some(":\n\n".into())
        );
    }

    #[tokio::test(start_paused = true)]
    async fn dropped_receiver_cancels_before_the_next_poll() {
        // Given
        let (sender, receiver) = mpsc::channel::<Result<Event, Infallible>>(1);
        drop(receiver);
        let mut interval = tokio::time::interval(POLL_INTERVAL);

        // When
        let open = wait_for_poll(&sender, &mut interval).await;

        // Then
        assert!(!open);
    }
}
