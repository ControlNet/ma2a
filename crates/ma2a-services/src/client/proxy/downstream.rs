//! Destination → local relay direction for Proxy v1.

use std::{io, sync::atomic::Ordering, time::Duration};

use ma2a_core::service::{Channel, EndReason, FrameKind, ProxyResult, ServiceError};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::Notify,
};
use tokio_util::sync::CancellationToken;

use super::Sent;
use crate::{
    FrameReader,
    client::{
        ClientError, decoded, next,
        sinks::{shutdown_bounded, write_bounded, write_until},
        unexpected,
    },
};

/// Destination → local until the outcome; a lost local side asks upstream to cancel, and
/// explicit cancellation interrupts local delivery and discards remaining output while still
/// checking frames and byte counts. Returns the answer and the first local write that failed.
///
/// The destination's bytes end once, and the outcome counts exactly the bytes received from it
/// and at most the `sent` ones as taken to it: an answer with other counts is not this relay.
/// A relay completes only once both sides ended, with every byte sent taken to the
/// destination; any other end may have moved part of the traffic.
pub(super) async fn downstream<R, LW>(
    reader: &mut FrameReader<R>,
    (mut local_write, idle): (LW, Duration),
    (lost, stop, sent): (&Notify, &CancellationToken, &Sent),
) -> (Result<ProxyResult, ClientError>, Option<io::Error>)
where
    R: AsyncRead + Unpin,
    LW: AsyncWrite + Unpin,
{
    let mut unwritten = None;
    let mut discarding = false;
    let answer = async {
        let invalid = || ClientError::Stream(ServiceError::InvalidInput);
        let (mut ended, mut received) = (false, 0_u64);
        loop {
            let frame = next(reader).await?;
            let delivered = match frame.kind() {
                FrameKind::Outcome => {
                    let result = decoded(ProxyResult::decode_outcome(frame.payload()))?;
                    let completed = result.reason == EndReason::Completed;
                    if completed && !(ended && sent.ended.load(Ordering::Acquire)) {
                        return Err(invalid());
                    }
                    let taken = if completed {
                        result.bytes_to_target == sent.completed.load(Ordering::Acquire)
                    } else {
                        result.bytes_to_target <= sent.started.load(Ordering::Acquire)
                    };
                    let counted = result.bytes_from_target == received && taken;
                    if !counted {
                        return Err(ClientError::Stream(ServiceError::IntegrityMismatch));
                    }
                    return Ok(result);
                }
                FrameKind::Data if !ended => match frame.data().map_err(ClientError::Stream)? {
                    (Channel::Input, bytes) => {
                        let length = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
                        received = received.saturating_add(length);
                        if unwritten.is_some() {
                            Some(Ok(()))
                        } else if discarding || stop.is_cancelled() {
                            None
                        } else {
                            write_until(write_bounded(&mut local_write, bytes, idle), stop).await
                        }
                    }
                    _ => return Err(invalid()),
                },
                FrameKind::End if !ended && frame.end() == Ok(Channel::Input) => {
                    ended = true;
                    if unwritten.is_some() {
                        Some(Ok(()))
                    } else if discarding || stop.is_cancelled() {
                        None
                    } else {
                        write_until(shutdown_bounded(&mut local_write, idle), stop).await
                    }
                }
                FrameKind::Data | FrameKind::End => return Err(invalid()),
                _ => return Err(unexpected(&frame)),
            };
            match delivered {
                Some(Ok(())) => {}
                Some(Err(failure)) => {
                    unwritten = Some(failure);
                    lost.notify_one();
                }
                None => discarding = true,
            }
        }
    }
    .await;
    (answer, unwritten)
}
