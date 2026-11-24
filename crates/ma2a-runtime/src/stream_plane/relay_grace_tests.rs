//! The relay's bounds on a target or client that stops answering, under a paused clock.

use ma2a_core::service::{FrameKind, ServiceError, failed_outcome};

use super::{
    Terminal,
    tests::{start, start_with},
};

#[tokio::test(start_paused = true)]
async fn a_cancelled_target_that_never_answers_is_lost_after_the_grace() {
    let ((_client_reader, mut client_writer), (mut target_reader, _target_writer), task) = start();
    client_writer
        .frame(FrameKind::Cancel, &[])
        .await
        .expect("cancel");
    assert_eq!(
        target_reader.expect().await.expect("cancel").kind(),
        FrameKind::Cancel
    );
    tokio::time::advance(super::CANCEL_GRACE).await;
    assert_eq!(task.await.expect("relay").terminal, Terminal::Lost);
}

#[tokio::test(start_paused = true)]
async fn a_terminal_the_client_does_not_take_is_given_up_after_the_final_write_grace() {
    // Given: a client that reads nothing, through a pipe with no room.
    let ((_client_reader, _client_writer), (_target_reader, mut target_writer), task) =
        start_with(1);

    // When: the target ends the operation.
    let started = tokio::time::Instant::now();
    let outcome = failed_outcome(ServiceError::Cancelled).expect("outcome");
    target_writer
        .frame(FrameKind::Outcome, &outcome)
        .await
        .expect("outcome");
    let relayed = task.await.expect("relay");

    // Then: the relay ends after the final-write grace, not the 30 s stall bound.
    assert!(started.elapsed() <= super::FINAL_WRITE_GRACE * 2);
    assert!(matches!(relayed.terminal, Terminal::Outcome { .. }));
}

#[tokio::test(start_paused = true)]
async fn a_terminal_read_within_the_grace_is_kept_when_the_client_never_takes_it() {
    // Given: a client that cancels and then reads nothing, through a pipe with no room.
    let ((_client_reader, mut client_writer), (mut target_reader, mut target_writer), task) =
        start_with(1);
    client_writer
        .frame(FrameKind::Cancel, &[])
        .await
        .expect("cancel");
    assert_eq!(
        target_reader.expect().await.expect("cancel").kind(),
        FrameKind::Cancel
    );

    // When: the target reports its terminal a second into the grace, which then runs out
    // while the terminal is still on its way to the client, before that write's own bound.
    tokio::time::advance(std::time::Duration::from_secs(1)).await;
    let outcome = failed_outcome(ServiceError::Cancelled).expect("outcome");
    target_writer
        .frame(FrameKind::Outcome, &outcome)
        .await
        .expect("outcome");
    let relayed = task.await.expect("relay");

    // Then: the terminal is known, and the client's stream is cut inside it.
    assert!(matches!(
        relayed.terminal,
        Terminal::Outcome {
            failure: Some(ServiceError::Cancelled),
            ..
        }
    ));
    assert!(!relayed.client_present);
}
