//! Inbound frame relay from one caller stream to one terminal session.

use std::sync::Mutex;

use ma2a_core::service::{Channel, FrameKind, ServiceError, TerminalSize};
use portable_pty::MasterPty;
use tokio::{
    io::AsyncRead,
    sync::{mpsc, oneshot},
};

use super::pty::pty_size;
use crate::{
    FrameReader,
    input::{Input, Wrote},
    report::len64,
};

/// Forwards input and resize requests until the session ends, returning input bytes.
pub(super) async fn inbound<R: AsyncRead + Unpin>(
    reader: &mut FrameReader<R>,
    (input, master): (mpsc::Sender<Vec<u8>>, &Mutex<Box<dyn MasterPty + Send>>),
    (cancel, mut finished): (oneshot::Sender<ServiceError>, oneshot::Receiver<()>),
) -> u64 {
    let mut reader = Input::new(reader);
    let mut input = Some(input);
    let mut input_bytes = 0_u64;
    // Set once the terminal stops taking input; the caller may not know yet and keeps sending.
    let mut closed_by_session = false;
    let failure = loop {
        let frame = tokio::select! {
            _ = &mut finished => return input_bytes,
            frame = reader.next() => frame,
        };
        let frame = match frame {
            Ok(Some(frame)) => frame,
            // A caller keeps its side open until the outcome; closing it earlier cancels.
            Ok(None) => break ServiceError::Cancelled,
            Err(error) => break error,
        };
        match frame.kind() {
            FrameKind::Data => {
                let Ok((Channel::Input, bytes)) = frame.data() else {
                    break ServiceError::InvalidInput;
                };
                let Some(sender) = input.as_ref() else {
                    if closed_by_session {
                        continue;
                    }
                    break ServiceError::InvalidInput;
                };
                input_bytes += len64(bytes.len());
                let chunk = bytes.to_vec();
                let sent = match reader.write(sender.send(chunk), &mut finished).await {
                    Wrote::Done(sent) => sent,
                    Wrote::Stopped(error) => break error,
                    Wrote::Finished => return input_bytes,
                };
                if sent.is_err() {
                    input = None;
                    closed_by_session = true;
                }
            }
            FrameKind::Control => match TerminalSize::decode_resize(frame.payload()) {
                Ok(size) => {
                    let _resized = master
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .resize(pty_size(size));
                }
                Err(_) => break ServiceError::InvalidInput,
            },
            FrameKind::End if frame.end() == Ok(Channel::Input) => {
                input = None;
                closed_by_session = false;
            }
            FrameKind::Cancel => break ServiceError::Cancelled,
            _ => break ServiceError::InvalidInput,
        }
    };
    let _signalled = cancel.send(failure);
    let _finished = finished.await;
    input_bytes
}

#[cfg(all(test, unix))]
#[path = "io_tests.rs"]
mod tests;
