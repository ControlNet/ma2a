//! Caller input relayed to a terminal that stopped taking it.

use std::{sync::Mutex, time::Duration};

use ma2a_core::service::{Channel, ServiceError, TerminalSize};
use portable_pty::native_pty_system;
use tokio::sync::{mpsc, oneshot};

use super::{super::pty::pty_size, inbound};
use crate::{FrameReader, FrameWriter};

#[tokio::test(start_paused = true)]
async fn input_after_the_terminal_stopped_taking_it_is_dropped_until_the_session_ends() {
    // Given: a terminal whose input side is gone, and a caller that keeps typing.
    let size = TerminalSize::new(80, 24).expect("size");
    let pair = native_pty_system().openpty(pty_size(size)).expect("pty");
    let master = Mutex::new(pair.master);
    let (input, gone) = mpsc::channel(1);
    drop(gone);
    let (one, two) = tokio::io::duplex(1 << 16);
    let ((_, caller), (read, _write)) = (tokio::io::split(one), tokio::io::split(two));
    let mut caller = FrameWriter::new(caller);
    for chunk in [&b"ls\n"[..], b"pwd\n", b"exit\n"] {
        caller.data(Channel::Input, chunk).await.expect("data");
    }
    let mut reader = FrameReader::new(read);
    let (cancel, mut cancelled) = oneshot::channel::<ServiceError>();
    let (done, finished) = oneshot::channel();

    // When: the session ends a moment later.
    let ending = async {
        tokio::time::sleep(Duration::from_millis(100)).await;
        done.send(()).expect("inbound is waiting");
    };
    let (input_bytes, ()) = tokio::join!(
        inbound(&mut reader, (input, &master), (cancel, finished)),
        ending
    );

    // Then: only the input offered to the terminal counts, and nothing ended the session early.
    assert_eq!(input_bytes, 3);
    assert!(cancelled.try_recv().is_err(), "the session was cancelled");
}
