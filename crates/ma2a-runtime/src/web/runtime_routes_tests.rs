use std::fmt::Write as _;

use axum::{
    body::to_bytes,
    response::{IntoResponse as _, Sse},
};
use tokio_stream::{StreamExt as _, wrappers::ReceiverStream};

use super::*;

#[test]
fn snapshot_change_classifies_consecutive_gap_and_restart() {
    // Given
    let cursor = EventCursor {
        revision: 7,
        boot_id: "00112233445566778899aabbccddeeff".to_owned(),
    };

    // When
    let unchanged = cursor.classify(&SnapshotStamp {
        revision: 7,
        boot_id: cursor.boot_id.clone(),
    });
    let consecutive = cursor.classify(&SnapshotStamp {
        revision: 8,
        boot_id: cursor.boot_id.clone(),
    });
    let gap = cursor.classify(&SnapshotStamp {
        revision: 9,
        boot_id: cursor.boot_id.clone(),
    });
    let restart = cursor.classify(&SnapshotStamp {
        revision: 8,
        boot_id: "ffeeddccbbaa99887766554433221100".to_owned(),
    });

    // Then
    assert_eq!(unchanged, SnapshotChange::Unchanged);
    assert_eq!(consecutive, SnapshotChange::Consecutive);
    assert_eq!(gap, SnapshotChange::Resync);
    assert_eq!(restart, SnapshotChange::Resync);
}

#[tokio::test]
async fn queue_overflow_reserves_terminal_resync_event() {
    // Given
    let boot_id = "00112233445566778899aabbccddeeff".to_owned();
    let mut cursor = EventCursor {
        revision: 10,
        boot_id: boot_id.clone(),
    };
    let (sender, receiver) = mpsc::channel(EVENT_CHANNEL_CAPACITY);

    // When
    for revision in 11..=18 {
        assert!(cursor.emit(
            &sender,
            &SnapshotStamp {
                revision,
                boot_id: boot_id.clone(),
            }
        ));
    }
    let remains_open = cursor.emit(
        &sender,
        &SnapshotStamp {
            revision: 19,
            boot_id,
        },
    );
    drop(sender);
    let response = Sse::new(ReceiverStream::new(receiver)).into_response();
    let events = to_bytes(response.into_body(), 16_384).await;
    assert!(events.is_ok(), "overflow event stream must serialize");
    let events = events.unwrap_or_default();
    let mut expected = String::new();
    for revision in 11..=18 {
        assert!(
            write!(
                &mut expected,
                "id: {revision}\ndata: {{\"changed\":{{}},\"revision\":{revision},\"type\":\"snapshot_invalidated\"}}\n\n"
            )
            .is_ok()
        );
    }
    expected.push_str("event: resync-required\ndata: {\"revision\":19}\n\n");

    // Then
    assert!(!remains_open);
    assert_eq!(events.as_ref(), expected.as_bytes());
}

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
    let frame = frame.transpose();
    assert!(frame.is_ok(), "heartbeat body must serialize");
    assert_eq!(frame.unwrap_or(None), Some(":\n\n".into()));
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
