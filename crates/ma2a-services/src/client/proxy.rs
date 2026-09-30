//! Caller side of Proxy v1: one local byte stream bound to one remote TCP destination.

use ma2a_core::service::{Channel, FrameKind, MAX_DATA_CHUNK_LEN, ProxyResult, ServiceError};
use tokio::{
    io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _},
    sync::Notify,
};

use super::{ClientError, First, decoded, first, next, unexpected};
use crate::{FrameReader, FrameWriter};

/// Relays bytes between a local stream and the target's destination until the outcome.
///
/// Local EOF becomes `end`, and the destination's `end` shuts down the local write side, so
/// each direction closes independently and byte for byte. The two directions never wait on
/// each other.
///
/// # Errors
/// Returns the refusal (`connection_refused`, `unreachable`, `dns_failed`, `timed_out`, or an
/// authorization failure), the failed outcome, or the stream failure.
pub async fn run<R, W, LR, LW>(
    (reader, writer): (&mut FrameReader<R>, &mut FrameWriter<W>),
    (local_read, local_write): (LR, LW),
) -> Result<ProxyResult, ClientError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
    LR: AsyncRead + Unpin,
    LW: AsyncWrite + Unpin,
{
    if let First::Finished(frame) = first(reader).await? {
        return decoded(ProxyResult::decode_outcome(frame.payload()));
    }
    let lost = Notify::new();
    let downstream = downstream(reader, local_write, &lost);
    let upstream = upstream(writer, local_read, &lost);
    tokio::pin!(downstream, upstream);
    tokio::select! {
        biased;
        result = &mut downstream => result,
        () = &mut upstream => downstream.await,
    }
}

/// Local → destination, then `end` at local EOF, and a `cancel` once the local side is lost.
async fn upstream<W, LR>(writer: &mut FrameWriter<W>, mut local_read: LR, lost: &Notify)
where
    W: AsyncWrite + Unpin,
    LR: AsyncRead + Unpin,
{
    let mut buffer = vec![0_u8; MAX_DATA_CHUNK_LEN];
    loop {
        let read = tokio::select! {
            biased;
            () = lost.notified() => {
                let _cancelled = cancel(writer).await;
                return;
            }
            read = local_read.read(&mut buffer) => read,
        };
        let Ok(read @ 1..) = read else {
            let ended = async {
                writer.end(Channel::Input).await?;
                writer.flush().await
            }
            .await;
            if ended.is_ok() {
                // The destination may keep sending; a lost local side still cancels it.
                lost.notified().await;
                let _cancelled = cancel(writer).await;
            }
            return;
        };
        let forwarded = writer
            .data(Channel::Input, buffer.get(..read).unwrap_or_default())
            .await;
        if forwarded.and(writer.flush().await).is_err() {
            return;
        }
    }
}

/// Destination → local until the outcome; a lost local side asks upstream to cancel.
async fn downstream<R, LW>(
    reader: &mut FrameReader<R>,
    mut local_write: LW,
    lost: &Notify,
) -> Result<ProxyResult, ClientError>
where
    R: AsyncRead + Unpin,
    LW: AsyncWrite + Unpin,
{
    let mut delivering = true;
    loop {
        let frame = next(reader).await?;
        let delivered = match frame.kind() {
            FrameKind::Outcome => return decoded(ProxyResult::decode_outcome(frame.payload())),
            FrameKind::Data => match frame.data().map_err(ClientError::Stream)? {
                (Channel::Input, bytes) => {
                    !delivering || write_flushed(&mut local_write, bytes).await.is_ok()
                }
                _ => return Err(ClientError::Stream(ServiceError::InvalidInput)),
            },
            FrameKind::End if frame.end() == Ok(Channel::Input) => {
                !delivering || local_write.shutdown().await.is_ok()
            }
            _ => return Err(unexpected(&frame)),
        };
        if !delivered {
            delivering = false;
            lost.notify_one();
        }
    }
}

async fn write_flushed<LW: AsyncWrite + Unpin>(sink: &mut LW, bytes: &[u8]) -> std::io::Result<()> {
    sink.write_all(bytes).await?;
    sink.flush().await
}

async fn cancel<W: AsyncWrite + Unpin>(writer: &mut FrameWriter<W>) -> Result<(), ServiceError> {
    writer.frame(FrameKind::Cancel, &[]).await?;
    writer.flush().await
}
