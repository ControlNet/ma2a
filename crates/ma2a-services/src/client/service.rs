//! Caller side of hosted services: discovery, and one call bound to local pipes.
//!
//! A call's frames follow Services v1 exactly: an optional `accept` with an opaque header,
//! then output and errors on channels 1 and 2 and control messages, until one `outcome`.

use std::future::Future;

use ma2a_core::service::{
    Channel, DescribePage, FrameKind, ServiceError, decode_call_accept, decode_call_outcome,
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::Notify,
};

use super::{
    ClientError, First, decoded, first_or_cancel, next,
    process::{ProcessControl, ProcessIo, finished, inbound, write_flushed},
    refused, unexpected,
};
use crate::{FrameReader, FrameWriter};

/// How one call ended successfully.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Called {
    /// The service's accept header; `None` when it answered without accepting.
    pub header: Option<Vec<u8>>,
    /// The result bytes of the outcome.
    pub result: Vec<u8>,
}

/// Reads one discovery page, answering a `describe` after `after`.
///
/// # Errors
/// Returns the refusal, such as `unauthorized` for an Endpoint sharing no Space, or the
/// stream failure: `invalid_input` for a page that does not continue the listing from `after`
/// (see [`DescribePage::continues`]).
pub async fn describe<R: AsyncRead + Unpin>(
    reader: &mut FrameReader<R>,
    after: Option<&str>,
) -> Result<DescribePage, ClientError> {
    let frame = next(reader).await?;
    let page = match frame.kind() {
        FrameKind::Outcome => decoded(DescribePage::decode_outcome(frame.payload()))?,
        FrameKind::Reject => return Err(refused(&frame)),
        _ => return Err(unexpected(&frame)),
    };
    if !page.continues(after) {
        return Err(ClientError::Stream(ServiceError::InvalidInput));
    }
    Ok(page)
}

/// Relays one call until its outcome: local input to channel 0 once the service accepted,
/// output and errors to their sinks, and one `cancel` when `cancel` completes or a local read
/// fails.
///
/// # Errors
/// Returns the refusal (`unknown_service`, `version_mismatch`, `concurrency_exceeded`, ...),
/// the failed outcome (`cancelled`, `outcome_unknown`, ...), the stream failure, or, once the
/// call was cancelled, the failed local read.
pub async fn call<R, W, I, O, E, C>(
    (reader, writer): (&mut FrameReader<R>, &mut FrameWriter<W>),
    local: ProcessIo<I, O, E>,
    cancel: C,
) -> Result<Called, ClientError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
    I: AsyncRead + Unpin,
    O: AsyncWrite + Unpin,
    E: AsyncWrite + Unpin,
    C: Future<Output = ()>,
{
    tokio::pin!(cancel);
    let (answer, cancelled) = first_or_cancel((reader, writer), cancel.as_mut()).await?;
    let header = match answer {
        First::Finished(frame) => {
            let result = decoded(decode_call_outcome(frame.payload()))?;
            return Ok(Called {
                header: None,
                result,
            });
        }
        First::Accepted(frame) => decode_call_accept(frame.payload())
            .map_err(|error| ClientError::Stream(ServiceError::from_protocol(error)))?,
    };
    let ProcessIo {
        input,
        output,
        error,
    } = local;
    let lost = Notify::new();
    let outbound = outbound(reader, (output, error), &lost);
    // Once cancelled, nothing more is sent; the outcome still follows.
    let result = if cancelled {
        outbound.await
    } else {
        let control = ProcessControl {
            resizes: None,
            cancel,
        };
        let inbound = inbound(writer, (input, control), &lost);
        tokio::pin!(outbound, inbound);
        tokio::select! {
            biased;
            result = &mut outbound => result,
            failed = &mut inbound => finished(outbound.await, failed),
        }
    }?;
    Ok(Called {
        header: Some(header),
        result,
    })
}

/// Service → caller until the outcome; lost local output asks the inbound side to cancel.
async fn outbound<R, O, E>(
    reader: &mut FrameReader<R>,
    (mut output, mut error): (O, Option<E>),
    lost: &Notify,
) -> Result<Vec<u8>, ClientError>
where
    R: AsyncRead + Unpin,
    O: AsyncWrite + Unpin,
    E: AsyncWrite + Unpin,
{
    let mut delivering = true;
    loop {
        let frame = next(reader).await?;
        match frame.kind() {
            FrameKind::Outcome => return decoded(decode_call_outcome(frame.payload())),
            FrameKind::Data => {
                let (channel, bytes) = frame.data().map_err(ClientError::Stream)?;
                let written = match (channel, error.as_mut()) {
                    (Channel::Input, _) => {
                        return Err(ClientError::Stream(ServiceError::InvalidInput));
                    }
                    _ if !delivering => Ok(()),
                    (Channel::Output, _) => write_flushed(&mut output, bytes).await,
                    (Channel::Error, Some(error)) => write_flushed(error, bytes).await,
                    (Channel::Error, None) => Ok(()),
                };
                if written.is_err() {
                    // Nothing can observe the call any more, so it is cancelled.
                    delivering = false;
                    lost.notify_one();
                }
            }
            // The service ends only its own channels.
            FrameKind::End if frame.end().map_err(ClientError::Stream)? == Channel::Input => {
                return Err(ClientError::Stream(ServiceError::InvalidInput));
            }
            // Control messages are the service's own; this generic caller has no use for them.
            FrameKind::End | FrameKind::Control => {}
            _ => return Err(unexpected(&frame)),
        }
    }
}
