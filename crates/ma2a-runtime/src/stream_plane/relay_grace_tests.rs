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

#[tokio::test(start_paused = true)]
async fn an_accept_the_client_does_not_take_is_given_up_after_the_final_write_grace() {
    // Given: a client that reads nothing, through a pipe with no room.
    let ((_client_reader, _client_writer), (mut target_reader, mut target_writer), _task) =
        start_with(1);

    // When: the target accepts the operation.
    let started = tokio::time::Instant::now();
    target_writer
        .frame(FrameKind::Accept, &[0xa0])
        .await
        .expect("accept");

    // Then: the target is cancelled after the final-write grace, not the 30 s stall bound.
    assert_eq!(
        target_reader.expect().await.expect("cancel").kind(),
        FrameKind::Cancel
    );
    assert_eq!(started.elapsed(), super::FINAL_WRITE_GRACE);
}

#[tokio::test(start_paused = true)]
async fn a_cancel_the_target_never_takes_ends_the_relay_without_a_second_grace() {
    // Given: a target that reads nothing, through a pipe with no room.
    let (client, (_target_reader, _target_writer), task) = start_with(1);

    // When: the client disappears, so the relay tries to cancel the target.
    let started = tokio::time::Instant::now();
    drop(client);
    let relayed = task.await.expect("relay");

    // Then: the grace spent on the undelivered cancel is the whole wait.
    assert_eq!(started.elapsed(), super::CANCEL_GRACE);
    assert_eq!(relayed.terminal, Terminal::Lost);
}

#[tokio::test(start_paused = true)]
async fn a_client_cancel_the_target_never_takes_ends_the_relay_after_the_cancel_grace() {
    // Given: a target that reads nothing, through a pipe with no room.
    let ((_client_reader, mut client_writer), (_target_reader, _target_writer), task) =
        start_with(1);

    // When: the client cancels in so many words rather than disappearing.
    let started = tokio::time::Instant::now();
    client_writer
        .frame(FrameKind::Cancel, &[])
        .await
        .expect("cancel");
    let relayed = task.await.expect("relay");

    // Then: the cancel has the same grace as one sent for a client that left.
    assert_eq!(started.elapsed(), super::CANCEL_GRACE);
    assert_eq!(relayed.terminal, Terminal::Lost);
}

#[tokio::test]
async fn a_continuous_downstream_flood_never_starves_client_cancel() {
    use ma2a_core::service::Channel;
    use ma2a_services::FrameWriter;
    use tokio::io::AsyncWriteExt as _;

    use super::{ClientWrite, relay, tests::ends};

    let (client, plane_client) = tokio::io::duplex(1 << 20);
    let (plane_target, target) = tokio::io::duplex(1 << 20);
    let (_client_reader, mut client_writer) = ends(client);
    let (mut target_reader, mut target_writer) = ends(target);
    let mut batch = FrameWriter::new(Vec::new());
    for _ in 0..256 {
        batch
            .data(Channel::Output, &[b'x'; 64])
            .await
            .expect("data");
    }
    let batch = batch.into_inner();
    let flood =
        tokio::spawn(
            async move { while target_writer.get_mut().write_all(&batch).await.is_ok() {} },
        );
    let task = tokio::spawn(async move {
        let ((mut client_reader, _), (mut target_reader, mut target_writer)) =
            (ends(plane_client), ends(plane_target));
        let mut sink = FrameWriter::new(tokio::io::sink());
        relay(
            (&mut client_reader, &mut sink),
            (&mut target_reader, &mut target_writer),
            (|| (), &ClientWrite::default()),
        )
        .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    client_writer
        .frame(FrameKind::Cancel, &[])
        .await
        .expect("cancel");
    let frame = tokio::time::timeout(std::time::Duration::from_secs(1), target_reader.expect())
        .await
        .expect("cancel must not starve")
        .expect("frame");
    assert_eq!(frame.kind(), FrameKind::Cancel);
    flood.abort();
    task.abort();
}
