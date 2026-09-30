//! Caller side of Exec v1 and Shell v1: local pipes or a terminal bound to a remote process.

use std::future::Future;

use ma2a_core::service::{
    Channel, FrameKind, MAX_DATA_CHUNK_LEN, ProcessResult, ServiceError, TerminalSize,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _},
    sync::{Notify, mpsc},
};

use super::{ClientError, First, decoded, first, next, unexpected};
use crate::{FrameReader, FrameWriter};

/// Local ends of one remote process.
#[expect(
    clippy::exhaustive_structs,
    reason = "a plain bundle of caller-owned ends, built by struct literal"
)]
#[derive(Debug)]
pub struct ProcessIo<I, O, E> {
    /// Forwarded as the process's input until EOF; `None` forwards nothing.
    pub input: Option<I>,
    /// Receives stdout (Exec) or terminal output (Shell).
    pub output: O,
    /// Receives stderr; Shell has none, and a target sending one breaks the contract.
    pub error: Option<E>,
}

/// Caller-side controls of one remote process.
#[expect(
    clippy::exhaustive_structs,
    reason = "a plain bundle of caller-owned controls, built by struct literal"
)]
#[derive(Debug)]
pub struct ProcessControl<C> {
    /// Terminal sizes to forward as `resize` controls (Shell only).
    pub resizes: Option<mpsc::Receiver<TerminalSize>>,
    /// Completes when the caller wants the process cancelled; the terminal still follows.
    pub cancel: C,
}

/// Relays one admitted remote process until its terminal outcome.
///
/// Input and output run independently, so a process that echoes a large input can never
/// deadlock against a caller that is still sending it.
///
/// # Errors
/// Returns the refusal (`spawn_failed` when the program cannot start), the failed outcome,
/// or the stream failure.
pub async fn run<R, W, I, O, E, C>(
    (reader, writer): (&mut FrameReader<R>, &mut FrameWriter<W>),
    local: ProcessIo<I, O, E>,
    control: ProcessControl<C>,
) -> Result<ProcessResult, ClientError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
    I: AsyncRead + Unpin,
    O: AsyncWrite + Unpin,
    E: AsyncWrite + Unpin,
    C: Future<Output = ()>,
{
    if let First::Finished(frame) = first(reader).await? {
        return decoded(ProcessResult::decode_outcome(frame.payload()));
    }
    let ProcessIo {
        input,
        output,
        error,
    } = local;
    let lost = Notify::new();
    let outbound = outbound(reader, (output, error), &lost);
    let inbound = inbound(writer, (input, control), &lost);
    tokio::pin!(outbound, inbound);
    tokio::select! {
        biased;
        result = &mut outbound => result,
        () = &mut inbound => outbound.await,
    }
}

/// Caller → target: input, resizes, and at most one `cancel`, until nothing is left to send.
async fn inbound<W, I, C>(
    writer: &mut FrameWriter<W>,
    (mut input, control): (Option<I>, ProcessControl<C>),
    lost: &Notify,
) where
    W: AsyncWrite + Unpin,
    I: AsyncRead + Unpin,
    C: Future<Output = ()>,
{
    let ProcessControl {
        mut resizes,
        cancel,
    } = control;
    tokio::pin!(cancel);
    let mut cancelled = false;
    let mut buffer = vec![0_u8; MAX_DATA_CHUNK_LEN];
    while input.is_some() || resizes.is_some() || !cancelled {
        let reading = input.is_some();
        let resizing = resizes.is_some();
        let stop = tokio::select! {
            biased;
            () = lost.notified(), if !cancelled => true,
            () = &mut cancel, if !cancelled => true,
            read = read_input(input.as_mut(), &mut buffer), if reading => {
                let forwarded = match read {
                    Some(length) => writer.data(Channel::Input, bytes_of(&buffer, length)).await,
                    None => writer.end(Channel::Input).await,
                };
                if read.is_none() || forwarded.and(writer.flush().await).is_err() {
                    input = None;
                }
                false
            }
            size = recv(resizes.as_mut()), if resizing => {
                match size.map(TerminalSize::encode_resize) {
                    Some(Ok(resize)) => send(writer, FrameKind::Control, &resize).await,
                    Some(Err(_)) => {}
                    None => resizes = None,
                }
                false
            }
        };
        if stop {
            cancelled = true;
            input = None;
            resizes = None;
            send(writer, FrameKind::Cancel, &[]).await;
        }
    }
}

/// Target → caller until the outcome; lost local output asks the inbound side to cancel.
async fn outbound<R, O, E>(
    reader: &mut FrameReader<R>,
    (mut output, mut error): (O, Option<E>),
    lost: &Notify,
) -> Result<ProcessResult, ClientError>
where
    R: AsyncRead + Unpin,
    O: AsyncWrite + Unpin,
    E: AsyncWrite + Unpin,
{
    let mut delivering = true;
    loop {
        let frame = next(reader).await?;
        match frame.kind() {
            FrameKind::Outcome => return decoded(ProcessResult::decode_outcome(frame.payload())),
            FrameKind::Data => {
                let (channel, bytes) = frame.data().map_err(ClientError::Stream)?;
                let written = match (channel, error.as_mut()) {
                    _ if !delivering => Ok(()),
                    (Channel::Output, _) => write_flushed(&mut output, bytes).await,
                    (Channel::Error, Some(error)) => write_flushed(error, bytes).await,
                    (Channel::Error, None) | (Channel::Input, _) => {
                        return Err(ClientError::Stream(ServiceError::InvalidInput));
                    }
                };
                if written.is_err() {
                    // Nothing can observe the process any more, so it is cancelled.
                    delivering = false;
                    lost.notify_one();
                }
            }
            FrameKind::End => {}
            _ => return Err(unexpected(&frame)),
        }
    }
}

async fn write_flushed<T: AsyncWrite + Unpin>(sink: &mut T, bytes: &[u8]) -> std::io::Result<()> {
    sink.write_all(bytes).await?;
    sink.flush().await
}

/// Reads local input; `None` is EOF, and a read failure ends input the same way.
async fn read_input<I: AsyncRead + Unpin>(
    input: Option<&mut I>,
    buffer: &mut [u8],
) -> Option<usize> {
    match input?.read(buffer).await {
        Ok(0) | Err(_) => None,
        Ok(read) => Some(read),
    }
}

fn bytes_of(buffer: &[u8], length: usize) -> &[u8] {
    buffer.get(..length).unwrap_or_default()
}

async fn recv(resizes: Option<&mut mpsc::Receiver<TerminalSize>>) -> Option<TerminalSize> {
    resizes?.recv().await
}

/// Sends one control frame; a failed write is answered by the target's terminal anyway.
async fn send<W: AsyncWrite + Unpin>(writer: &mut FrameWriter<W>, kind: FrameKind, payload: &[u8]) {
    let _sent = async {
        writer.frame(kind, payload).await?;
        writer.flush().await
    }
    .await;
}
