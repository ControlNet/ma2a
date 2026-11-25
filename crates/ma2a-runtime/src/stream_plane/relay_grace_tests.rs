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
            (|| (), &ClientWrite::default(), super::FORWARD_STALL),
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

fn start_asymmetric(
    (client_cap, target_cap): (usize, usize),
    stall: std::time::Duration,
) -> (
    (super::tests::Reader, super::tests::Writer),
    (super::tests::Reader, super::tests::Writer),
    tokio::task::JoinHandle<super::Relayed>,
) {
    let (client, plane_client) = tokio::io::duplex(client_cap);
    let (plane_target, target) = tokio::io::duplex(target_cap);
    let task = tokio::spawn(async move {
        let ((mut cr, mut cw), (mut tr, mut tw)) = (
            super::tests::ends(plane_client),
            super::tests::ends(plane_target),
        );
        super::relay(
            (&mut cr, &mut cw),
            (&mut tr, &mut tw),
            (|| (), &super::ClientWrite::default(), stall),
        )
        .await
    });
    (super::tests::ends(client), super::tests::ends(target), task)
}

#[tokio::test(start_paused = true)]
async fn a_proxy_relay_waits_past_forward_stall_for_its_idle_timeout() {
    use ma2a_core::service::{Channel, EndReason, OpenBody, ProxyHost, ProxyRequest, ProxyResult};

    let request = ProxyRequest::new(ProxyHost::new("127.0.0.1").expect("host"), 80, 40_000)
        .expect("proxy request");
    let stall = crate::stream_plane::stall_for(&OpenBody::Proxy(request));

    // 1. Blocked upstream write waits 40 s (past 30 s FORWARD_STALL) for target TimedOut.
    let ((mut cr, mut cw), (_tr, mut tw), task) = start_asymmetric((1 << 16, 1), stall);
    cw.data(Channel::Input, b"blocked").await.expect("data");
    cw.flush().await.expect("flush");
    tokio::task::yield_now().await;
    tokio::time::advance(std::time::Duration::from_secs(40)).await;
    let timed_out =
        ProxyResult::encode_outcome(&Ok(ProxyResult::new(EndReason::TimedOut, (0, 0), 40_000)))
            .expect("outcome");
    let _sent = tw.frame(FrameKind::Outcome, &timed_out).await;
    let relayed = task.await.expect("relay");
    let _drained = cr.next().await;
    assert!(
        matches!(relayed.terminal, Terminal::Outcome { failure: None, .. }),
        "expected target TimedOut outcome, got {:?}",
        relayed.terminal
    );

    // 2. Slow local Proxy consumer draining after 35 s (< 40 s idle) still gets every frame.
    let ((mut cr, _cw), (_tr, mut tw), task) = start_asymmetric((1, 1 << 16), stall);
    tw.data(Channel::Input, b"slow-bytes").await.expect("data");
    let completed =
        ProxyResult::encode_outcome(&Ok(ProxyResult::new(EndReason::Completed, (0, 10), 35_000)))
            .expect("outcome");
    tw.frame(FrameKind::Outcome, &completed)
        .await
        .expect("outcome");
    tw.flush().await.expect("flush");
    tokio::task::yield_now().await;
    tokio::time::advance(std::time::Duration::from_secs(35)).await;
    assert_eq!(
        cr.expect().await.expect("data").data(),
        Ok((Channel::Input, &b"slow-bytes"[..]))
    );
    assert_eq!(
        cr.expect().await.expect("outcome").kind(),
        FrameKind::Outcome
    );
    let relayed = task.await.expect("relay");
    assert!(relayed.client_present);
    assert!(matches!(
        relayed.terminal,
        Terminal::Outcome { failure: None, .. }
    ));
}

#[tokio::test(start_paused = true)]
async fn a_proxy_relay_cancels_within_cancel_grace_when_client_cancels_or_leaves() {
    use ma2a_core::service::Channel;

    for disconnect in [false, true] {
        let ((cr, mut cw), _target, task) =
            start_asymmetric((1 << 16, 1), std::time::Duration::from_secs(605));
        cw.data(Channel::Input, b"blocked").await.expect("data");
        cw.flush().await.expect("flush");
        tokio::task::yield_now().await;
        let started = tokio::time::Instant::now();
        if disconnect {
            drop((cr, cw));
        } else {
            cw.frame(FrameKind::Cancel, &[]).await.expect("cancel");
            cw.flush().await.expect("flush");
        }
        let relayed = task.await.expect("relay");
        assert_eq!(started.elapsed(), super::CANCEL_GRACE);
        assert_eq!(relayed.terminal, Terminal::Lost);
        assert_eq!(relayed.client_present, !disconnect);
    }
}
