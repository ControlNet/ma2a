//! Caller frames read while a write of caller input waits, under a paused clock.

use ma2a_core::service::{Channel, FrameKind, ServiceError};
use tokio::{
    io::{DuplexStream, ReadHalf, WriteHalf},
    sync::oneshot,
};

use super::{INPUT_STALL, Input, Wrote};
use crate::{FrameReader, FrameWriter};

type Ends = (
    FrameReader<ReadHalf<DuplexStream>>,
    FrameWriter<WriteHalf<DuplexStream>>,
);

fn caller() -> (FrameWriter<WriteHalf<DuplexStream>>, Ends) {
    let (one, two) = tokio::io::duplex(1 << 18);
    let ((_, caller), (read, write)) = (tokio::io::split(one), tokio::io::split(two));
    (
        FrameWriter::new(caller),
        (FrameReader::new(read), FrameWriter::new(write)),
    )
}

#[tokio::test(start_paused = true)]
async fn a_cancel_behind_a_write_the_program_never_takes_stops_it_at_once() {
    let (mut caller, (mut reader, _writer)) = caller();
    caller.frame(FrameKind::Cancel, &[]).await.expect("cancel");
    let (_done, mut finished) = oneshot::channel::<()>();
    let started = tokio::time::Instant::now();

    let wrote = Input::new(&mut reader)
        .write(std::future::pending::<()>(), &mut finished)
        .await;

    assert_eq!(wrote, Wrote::Stopped(ServiceError::Cancelled));
    assert_eq!(started.elapsed(), std::time::Duration::ZERO);
}

#[tokio::test(start_paused = true)]
async fn a_write_with_input_held_behind_it_is_stopped_after_the_stall_bound() {
    let (mut caller, (mut reader, _writer)) = caller();
    caller.data(Channel::Input, b"more").await.expect("data");
    caller.frame(FrameKind::Cancel, &[]).await.expect("cancel");
    let (_done, mut finished) = oneshot::channel::<()>();
    let started = tokio::time::Instant::now();

    let mut input = Input::new(&mut reader);
    let wrote = input
        .write(std::future::pending::<()>(), &mut finished)
        .await;

    assert_eq!(wrote, Wrote::Stopped(ServiceError::Cancelled));
    assert_eq!(started.elapsed(), INPUT_STALL);
    let held = input.next().await.expect("held").expect("frame");
    assert_eq!(held.data().expect("data"), (Channel::Input, &b"more"[..]));
}

#[tokio::test(start_paused = true)]
async fn a_stream_closed_behind_a_waiting_write_stops_it_at_once() {
    // A caller keeps its write side open until the terminal, so closing it is a cancel, also
    // after the end of its input.
    for ended in [false, true] {
        let (mut caller, (mut reader, _writer)) = caller();
        if ended {
            caller.end(Channel::Input).await.expect("end");
        }
        drop(caller);
        let (_done, mut finished) = oneshot::channel::<()>();
        let started = tokio::time::Instant::now();

        let wrote = Input::new(&mut reader)
            .write(std::future::pending::<()>(), &mut finished)
            .await;

        assert_eq!(wrote, Wrote::Stopped(ServiceError::Cancelled), "{ended}");
        assert_eq!(started.elapsed(), std::time::Duration::ZERO);
    }
}

#[tokio::test(start_paused = true)]
async fn the_end_of_input_behind_a_write_lets_a_slow_program_take_it() {
    let (mut caller, (mut reader, _writer)) = caller();
    caller.end(Channel::Input).await.expect("end");
    caller.flush().await.expect("flush");
    let (done, mut finished) = oneshot::channel::<()>();
    let taken = async {
        tokio::time::sleep(INPUT_STALL * 2).await;
        let _sent = done.send(());
    };

    let mut input = Input::new(&mut reader);
    let (wrote, ()) = tokio::join!(
        input.write(std::future::pending::<()>(), &mut finished),
        taken
    );

    assert_eq!(wrote, Wrote::Finished);
    let held = input.next().await.expect("held").expect("frame");
    assert_eq!(held.end(), Ok(Channel::Input));
}

#[tokio::test(start_paused = true)]
async fn a_cancel_after_the_end_of_input_stops_a_waiting_write_at_once() {
    let (mut caller, (mut reader, _writer)) = caller();
    caller.end(Channel::Input).await.expect("end");
    caller.frame(FrameKind::Cancel, &[]).await.expect("cancel");
    let (_done, mut finished) = oneshot::channel::<()>();
    let started = tokio::time::Instant::now();

    let wrote = Input::new(&mut reader)
        .write(std::future::pending::<()>(), &mut finished)
        .await;

    assert_eq!(wrote, Wrote::Stopped(ServiceError::Cancelled));
    assert_eq!(started.elapsed(), std::time::Duration::ZERO);
}

#[tokio::test(start_paused = true)]
async fn a_bounded_write_sees_cancel_or_close_at_once_and_keeps_its_own_bound() {
    let bound = INPUT_STALL * 2;
    for case in ["cancel", "end-then-cancel", "eof", "end-then-eof"] {
        let (mut caller, (mut reader, _writer)) = caller();
        if case.starts_with("end-") {
            caller.end(Channel::Input).await.expect("end");
        }
        if case.ends_with("cancel") {
            caller.frame(FrameKind::Cancel, &[]).await.expect("cancel");
        } else {
            drop(caller);
        }
        let started = tokio::time::Instant::now();
        let wrote = Input::new(&mut reader)
            .write_within(std::future::pending::<()>(), bound)
            .await;
        assert_eq!(wrote, Some(Err(ServiceError::Cancelled)), "{case}");
        assert_eq!(started.elapsed(), std::time::Duration::ZERO, "{case}");
    }

    // A held data frame does not shorten the write bound to `INPUT_STALL`.
    let (mut client, (mut reader, _writer)) = caller();
    client.data(Channel::Input, b"more").await.expect("data");
    client.flush().await.expect("flush");
    let started = tokio::time::Instant::now();
    let mut input = Input::new(&mut reader);
    let wrote = input
        .write_within(std::future::pending::<()>(), bound)
        .await;
    assert_eq!(wrote, None);
    assert_eq!(started.elapsed(), bound);
    let held = input.next().await.expect("held").expect("frame");
    assert_eq!(held.data().expect("data"), (Channel::Input, &b"more"[..]));

    // A frame other than `cancel` after `end` violates the contract at once.
    let (mut client, (mut reader, _writer)) = caller();
    client.end(Channel::Input).await.expect("end");
    client.data(Channel::Input, b"late").await.expect("late");
    let started = tokio::time::Instant::now();
    let wrote = Input::new(&mut reader)
        .write_within(std::future::pending::<()>(), bound)
        .await;
    assert_eq!(wrote, Some(Err(ServiceError::InvalidInput)));
    assert_eq!(started.elapsed(), std::time::Duration::ZERO);
}
