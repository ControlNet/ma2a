//! The stream's follow decisions and its client-disconnect handling.

use std::{convert::Infallible, time::Duration};

use axum::response::sse::Event;
use serde_json::json;
use tokio::sync::mpsc;

use super::{Next, Position, next, while_connected};
use crate::a2a::{backend::ended, view::settled};

#[test]
fn a_continued_stream_closes_on_a_terminal_first_frame() {
    // Given: a message sent at event 2 to a task that completed before the first frame.
    let position = Position::new(("completed".to_owned(), 1_000), settled, 5);

    // Then: nothing can follow the end.
    assert!(position.closes_at_once(Some(2)));
    let waiting = Position::new(("waiting".to_owned(), 1_000), settled, 5);
    assert!(!waiting.closes_at_once(Some(2)));
    assert!(waiting.closes_at_once(None));
}

#[test]
fn events_the_first_frame_showed_are_not_sent_again() {
    // Given: a message sent at event 2; by the first frame, at event 4, the task took it
    // and waits again.
    let mut position = Position::new(("waiting".to_owned(), 1_000), settled, 4);
    let state = |seq: u64, state: &str| json!({"seq": seq, "kind": "state", "state": state});

    // When: the follow replays from the message.
    let running = position.forward(&state(3, "running"), ("t", "b"));
    let waiting = position.forward(&state(4, "waiting"), ("t", "b"));

    // Then: neither is sent, and the stream closes on the wait the frame showed.
    assert_eq!(running, (None, false));
    assert_eq!(waiting, (None, true));
    let mut subscribed = Position::new(("running".to_owned(), 1_000), ended, 4);
    let live = subscribed.forward(&state(5, "completed"), ("t", "b"));
    assert!(live.0.is_some() && live.1);
}

#[test]
fn a_wait_the_first_frame_no_longer_showed_does_not_close_the_stream() {
    // Given: a message sent at event 5; by the first frame, at event 10, the task waited at
    // event 8, took another message, and runs again.
    let mut position = Position::new(("running".to_owned(), 1_000), settled, 10);
    let state = |seq: u64, state: &str| json!({"seq": seq, "kind": "state", "state": state});

    // When: the follow replays from the message.
    let superseded = position.forward(&state(8, "waiting"), ("t", "b"));
    let running = position.forward(&state(10, "running"), ("t", "b"));
    let waiting = position.forward(&state(11, "waiting"), ("t", "b"));

    // Then: only the wait after the frame closes the stream.
    assert_eq!(superseded, (None, false));
    assert_eq!(running, (None, false));
    assert!(waiting.0.is_some() && waiting.1);
}

#[test]
fn progress_and_agent_messages_keep_the_last_state_timestamp_across_replay() {
    // Given: a task whose last state change before `shown` was at 1_500 ms.
    let mut position = Position::new(("running".to_owned(), 1_500), ended, 4);
    let stamp = |update: Option<serde_json::Value>| {
        update
            .and_then(|value| value.pointer("/statusUpdate/status/timestamp").cloned())
            .and_then(|value| value.as_str().map(str::to_owned))
    };

    // Replayed progress before `shown` leaves the snapshot's state timestamp intact.
    let replayed = position.forward(
        &json!({"seq": 4, "at_ms": 1_800, "kind": "progress", "text": "old"}),
        ("t", "b"),
    );
    assert_eq!(replayed, (None, false));

    // Live progress and an Agent message after `shown` keep 1_500 ms; a real transition moves it.
    let progress = position.forward(
        &json!({"seq": 5, "at_ms": 2_000, "kind": "progress", "text": "half"}),
        ("t", "b"),
    );
    let message = position.forward(
        &json!({
            "seq": 6, "at_ms": 2_500, "kind": "message",
            "message": {"role": "agent", "parts": [{"kind": "text", "content": "hi"}]},
        }),
        ("t", "b"),
    );
    let waiting = position.forward(
        &json!({"seq": 7, "at_ms": 3_000, "kind": "state", "state": "waiting"}),
        ("t", "b"),
    );
    let at = crate::a2a::time::format;
    assert_eq!(stamp(progress.0), Some(at(1_500)));
    assert_eq!(stamp(message.0), Some(at(1_500)));
    assert_eq!(stamp(waiting.0), Some(at(3_000)));
}

#[tokio::test]
async fn a_disconnected_client_ends_the_follow_of_a_quiet_task() {
    // Given: a follow that never ends and no event to forward.
    let (frames, client) = mpsc::channel::<Result<Event, Infallible>>(1);
    let (_events, mut received) = mpsc::unbounded_channel();
    let following = std::pin::pin!(std::future::pending());

    // When
    drop(client);
    let next = tokio::time::timeout(
        Duration::from_secs(5),
        next(&mut received, following, &frames),
    )
    .await;

    // Then
    assert!(matches!(next, Ok(Next::Gone)));
}

#[tokio::test(start_paused = true)]
async fn a_disconnected_client_ends_a_stream_still_reading_its_task() {
    // Given: a first read of the task that never answers.
    let (frames, client) = mpsc::channel::<Result<Event, Infallible>>(1);

    // When
    drop(client);
    let ended = tokio::time::timeout(
        Duration::from_secs(5),
        while_connected(&frames, std::future::pending::<()>()),
    )
    .await;

    // Then
    assert!(ended.is_ok());
}
