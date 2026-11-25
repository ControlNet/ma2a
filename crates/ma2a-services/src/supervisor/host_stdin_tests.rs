//! Daemon frames read while a stdin write waits, under a paused clock.

use ma2a_core::service::{Channel, FrameKind, ServiceError};
use tokio::io::{DuplexStream, ReadHalf};

use super::{Daemon, INPUT_STALL};
use crate::{FrameReader, FrameWriter};

/// The daemon's writer, and the supervisor's reader of what it writes.
fn connection() -> (
    FrameWriter<tokio::io::WriteHalf<DuplexStream>>,
    FrameReader<ReadHalf<DuplexStream>>,
) {
    let (one, two) = tokio::io::duplex(1 << 18);
    let ((_, daemon), (read, _)) = (tokio::io::split(one), tokio::io::split(two));
    (FrameWriter::new(daemon), FrameReader::new(read))
}

/// A write to a child that never reads its stdin.
async fn blocked() -> Result<(), ServiceError> {
    std::future::pending().await
}

#[tokio::test(start_paused = true)]
async fn a_cancel_or_a_closed_connection_abandons_a_blocked_write_at_once() {
    for cancel in [true, false] {
        let (mut daemon, mut reader) = connection();
        daemon.end(Channel::Input).await.expect("end");
        if cancel {
            daemon.frame(FrameKind::Cancel, &[]).await.expect("cancel");
        } else {
            drop(daemon);
        }
        let started = tokio::time::Instant::now();

        let mut input = Daemon::new(&mut reader);
        let written = input.write(blocked()).await;

        assert_eq!(written, Err(ServiceError::Cancelled), "cancel {cancel}");
        assert_eq!(started.elapsed(), std::time::Duration::ZERO);
    }
}

#[tokio::test(start_paused = true)]
async fn a_write_with_input_held_behind_it_times_out_after_the_stall_bound() {
    let (mut daemon, mut reader) = connection();
    daemon.data(Channel::Input, b"more").await.expect("data");
    daemon.frame(FrameKind::Cancel, &[]).await.expect("cancel");
    let started = tokio::time::Instant::now();

    let mut input = Daemon::new(&mut reader);
    let written = input.write(blocked()).await;

    assert_eq!(written, Err(ServiceError::TimedOut));
    assert_eq!(started.elapsed(), INPUT_STALL);
    let held = input.next().await.expect("held").expect("frame");
    assert_eq!(held.data().expect("data"), (Channel::Input, &b"more"[..]));
}

#[tokio::test(start_paused = true)]
async fn a_write_that_finishes_keeps_the_frames_read_meanwhile() {
    let (mut daemon, mut reader) = connection();
    daemon.end(Channel::Input).await.expect("end");
    let mut input = Daemon::new(&mut reader);
    let written = input
        .write(async {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            Ok(())
        })
        .await;

    assert_eq!(written, Ok(Ok(())));
    let ended = input.next().await.expect("ended").expect("frame");
    assert_eq!(ended.end().expect("end"), Channel::Input);
}

#[tokio::test(start_paused = true)]
async fn a_write_with_nothing_or_only_the_end_behind_it_times_out_after_the_stall_bound() {
    for end in [false, true] {
        let (mut daemon, mut reader) = connection();
        if end {
            daemon.end(Channel::Input).await.expect("end");
        }
        let started = tokio::time::Instant::now();

        let mut input = Daemon::new(&mut reader);
        let written = input.write(blocked()).await;

        assert_eq!(written, Err(ServiceError::TimedOut), "end {end}");
        assert_eq!(started.elapsed(), INPUT_STALL);
        if end {
            let ended = input.next().await.expect("ended").expect("frame");
            assert_eq!(ended.end().expect("end"), Channel::Input);
        }
        drop(daemon);
    }
}

#[tokio::test(start_paused = true)]
async fn a_write_that_finishes_just_within_the_stall_bound_is_written() {
    let (_daemon, mut reader) = connection();
    let written = Daemon::new(&mut reader)
        .write(async {
            tokio::time::sleep(INPUT_STALL - std::time::Duration::from_millis(1)).await;
            Ok(())
        })
        .await;

    assert_eq!(written, Ok(Ok(())));
}
