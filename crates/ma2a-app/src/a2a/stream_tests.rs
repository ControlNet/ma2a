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
    let position = Position {
        current: "completed".to_owned(),
        finishes: settled,
        shown: 5,
    };

    // Then: nothing can follow the end.
    assert!(position.closes_at_once(Some(2)));
    let waiting = Position {
        current: "waiting".to_owned(),
        ..position
    };
    assert!(!waiting.closes_at_once(Some(2)));
    assert!(waiting.closes_at_once(None));
}

#[test]
fn events_the_first_frame_showed_are_not_sent_again() {
    // Given: a message sent at event 2; by the first frame, at event 4, the task took it
    // and waits again.
    let mut position = Position {
        current: "waiting".to_owned(),
        finishes: settled,
        shown: 4,
    };
    let state = |seq: u64, state: &str| json!({"seq": seq, "kind": "state", "state": state});

    // When: the follow replays from the message.
    let running = position.forward(&state(3, "running"), ("t", "b"));
    let waiting = position.forward(&state(4, "waiting"), ("t", "b"));

    // Then: neither is sent, and the stream closes on the wait the frame showed.
    assert_eq!(running, (None, false));
    assert_eq!(waiting, (None, true));
    let mut subscribed = Position {
        current: "running".to_owned(),
        finishes: ended,
        shown: 4,
    };
    let live = subscribed.forward(&state(5, "completed"), ("t", "b"));
    assert!(live.0.is_some() && live.1);
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
