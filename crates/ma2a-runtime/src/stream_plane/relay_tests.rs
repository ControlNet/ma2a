//! The relay against in-memory pipes standing in for the IPC client and the QUIC target.

use ma2a_core::service::{Channel, FrameKind, ServiceError, failed_outcome};
use ma2a_services::{FrameReader, FrameWriter};
use tokio::io::{DuplexStream, ReadHalf, WriteHalf};

use super::{Relayed, Terminal, relay};

type Reader = FrameReader<ReadHalf<DuplexStream>>;
type Writer = FrameWriter<WriteHalf<DuplexStream>>;

fn ends(stream: DuplexStream) -> (Reader, Writer) {
    let (read, write) = tokio::io::split(stream);
    (FrameReader::new(read), FrameWriter::new(write))
}

type Started = (
    (Reader, Writer),
    (Reader, Writer),
    tokio::task::JoinHandle<Relayed>,
);

/// Starts a relay; returns the test's client end, target end, and the relay's result.
fn start() -> Started {
    start_with(1 << 18)
}

/// Starts a relay whose pipes hold at most `capacity` bytes in flight.
fn start_with(capacity: usize) -> Started {
    let (client, plane_client) = tokio::io::duplex(capacity);
    let (plane_target, target) = tokio::io::duplex(capacity);
    let task = tokio::spawn(async move {
        let (mut client_reader, mut client_writer) = ends(plane_client);
        let (mut target_reader, mut target_writer) = ends(plane_target);
        relay(
            (&mut client_reader, &mut client_writer),
            (&mut target_reader, &mut target_writer),
            (|| (), &super::ClientWrite::default()),
        )
        .await
    });
    (ends(client), ends(target), task)
}

#[tokio::test]
async fn frames_pass_verbatim_both_ways_until_the_outcome() {
    let ((mut client_reader, mut client_writer), (mut target_reader, mut target_writer), task) =
        start();
    client_writer
        .data(Channel::Input, b"stdin")
        .await
        .expect("client data");
    client_writer.end(Channel::Input).await.expect("client end");
    let data = target_reader.expect().await.expect("target data");
    assert_eq!(
        data.data().expect("data"),
        (Channel::Input, b"stdin".as_slice())
    );
    assert_eq!(
        target_reader.expect().await.expect("end").kind(),
        FrameKind::End
    );

    target_writer
        .frame(FrameKind::Accept, &[0xa0])
        .await
        .expect("accept");
    target_writer
        .data(Channel::Output, b"stdout!")
        .await
        .expect("output");
    let outcome = failed_outcome(ServiceError::NotFound).expect("outcome");
    target_writer
        .frame(FrameKind::Outcome, &outcome)
        .await
        .expect("outcome");
    assert_eq!(
        client_reader.expect().await.expect("accept").kind(),
        FrameKind::Accept
    );
    assert_eq!(
        client_reader
            .expect()
            .await
            .expect("output")
            .data()
            .expect("data"),
        (Channel::Output, b"stdout!".as_slice())
    );
    let terminal = client_reader.expect().await.expect("terminal");
    assert_eq!(terminal.payload(), outcome.as_slice());

    let relayed = task.await.expect("relay");
    assert_eq!((relayed.to_target, relayed.from_target), (5, 7));
    assert!(relayed.client_present);
    let Terminal::Outcome { frame, failure } = relayed.terminal else {
        panic!("expected an outcome, got {:?}", relayed.terminal);
    };
    assert_eq!(failure, Some(ServiceError::NotFound));
    assert_eq!(
        frame,
        super::encoded(FrameKind::Outcome, &outcome).expect("frame")
    );
}

#[tokio::test]
async fn a_target_reject_is_forwarded_and_reported_as_not_started() {
    let ((mut client_reader, _client_writer), (_target_reader, mut target_writer), task) = start();
    target_writer
        .reject(ServiceError::Unauthorized)
        .await
        .expect("reject");
    let frame = client_reader.expect().await.expect("reject frame");
    assert_eq!(frame.rejection(), Ok(ServiceError::Unauthorized));
    assert_eq!(
        task.await.expect("relay").terminal,
        Terminal::Rejected(ServiceError::Unauthorized)
    );
}

#[tokio::test]
async fn a_reject_after_accept_is_indeterminate_and_never_forwarded() {
    let ((mut client_reader, _client_writer), (_target_reader, mut target_writer), task) = start();
    target_writer
        .frame(FrameKind::Accept, &[0xa0])
        .await
        .expect("accept");
    target_writer
        .reject(ServiceError::Unauthorized)
        .await
        .expect("reject");
    assert_eq!(task.await.expect("relay").terminal, Terminal::Lost);
    let forwarded = client_reader.expect().await.expect("accept frame");
    assert_eq!(forwarded.kind(), FrameKind::Accept);
    assert!(
        client_reader
            .next()
            .await
            .map_or(true, |frame| frame.is_none())
    );
}

#[tokio::test]
async fn a_target_lost_before_its_terminal_is_indeterminate() {
    let (_client, target, task) = start();
    drop(target);
    assert_eq!(task.await.expect("relay").terminal, Terminal::Lost);
}

#[tokio::test]
async fn a_vanished_client_cancels_the_target_which_still_reports_its_terminal() {
    let (client, (mut target_reader, mut target_writer), task) = start();
    drop(client);
    assert_eq!(
        target_reader.expect().await.expect("cancel").kind(),
        FrameKind::Cancel
    );
    let outcome = failed_outcome(ServiceError::Cancelled).expect("outcome");
    target_writer
        .frame(FrameKind::Outcome, &outcome)
        .await
        .expect("outcome");
    let relayed = task.await.expect("relay");
    assert!(!relayed.client_present);
    assert!(matches!(
        relayed.terminal,
        Terminal::Outcome {
            failure: Some(ServiceError::Cancelled),
            ..
        }
    ));
}

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

#[tokio::test]
async fn a_client_protocol_violation_cancels_the_target() {
    let ((_client_reader, mut client_writer), (mut target_reader, _target_writer), _task) = start();
    client_writer
        .frame(FrameKind::Open, &[0xa0])
        .await
        .expect("second open");
    assert_eq!(
        target_reader.expect().await.expect("cancel").kind(),
        FrameKind::Cancel
    );
}

/// Sends `count` numbered data frames, each `length` bytes of its own number.
async fn send_numbered(writer: &mut Writer, channel: Channel, (count, length): (u8, usize)) {
    for number in 0..count {
        writer
            .data(channel, &vec![number; length])
            .await
            .expect("numbered data");
    }
    writer.flush().await.expect("flush");
}

/// Receives `count` numbered data frames and checks each arrived whole and in order.
async fn receive_numbered(reader: &mut Reader, (count, length): (u8, usize)) {
    for number in 0..count {
        let frame = reader.expect().await.expect("numbered frame");
        let (_, bytes) = frame.data().expect("data");
        assert_eq!(bytes, vec![number; length].as_slice());
    }
}

#[tokio::test]
async fn concurrent_traffic_both_ways_through_tiny_pipes_arrives_whole_and_in_order() {
    // Tiny pipes split every frame across many reads, so a relay that lost a partially read
    // frame whenever the other direction won the race would corrupt both streams.
    let ((mut client_reader, mut client_writer), (mut target_reader, mut target_writer), task) =
        start_with(7);
    let shape = (60, 3_000);
    target_writer
        .frame(FrameKind::Accept, &[0xa0])
        .await
        .expect("accept");
    let downstream = async {
        assert_eq!(
            client_reader.expect().await.expect("accept").kind(),
            FrameKind::Accept
        );
        receive_numbered(&mut client_reader, shape).await;
    };
    tokio::join!(
        send_numbered(&mut client_writer, Channel::Input, shape),
        receive_numbered(&mut target_reader, shape),
        send_numbered(&mut target_writer, Channel::Output, shape),
        downstream,
    );
    let outcome = failed_outcome(ServiceError::Cancelled).expect("outcome");
    target_writer
        .frame(FrameKind::Outcome, &outcome)
        .await
        .expect("outcome");
    target_writer.flush().await.expect("flush");
    assert_eq!(
        client_reader.expect().await.expect("outcome").kind(),
        FrameKind::Outcome
    );
    let relayed = task.await.expect("relay");
    assert_eq!((relayed.to_target, relayed.from_target), (180_000, 180_000));
}
