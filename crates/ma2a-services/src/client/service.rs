//! Caller side of hosted services: discovery, and one call bound to local pipes.
//!
//! A call's frames follow Services v1 exactly: an optional `accept` with an opaque header,
//! then output and errors on channels 1 and 2 and control messages, until one `outcome`.

use std::future::Future;

use ma2a_core::service::{
    DescribePage, FrameKind, ServiceError, decode_call_accept, decode_call_outcome,
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::Notify,
};
use tokio_util::sync::CancellationToken;

use super::{
    ClientError, First, Sender, decoded, drained, first_or_cancel, next,
    process::{ProcessControl, ProcessIo, inbound},
    refused,
    sinks::{Sinks, settled},
    unexpected,
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
    /// Whether local delivery to the caller's sinks was interrupted or discarded.
    pub delivery_interrupted: bool,
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
    W: AsyncWrite + Unpin + Send,
    I: AsyncRead + Unpin,
    O: AsyncWrite + Unpin,
    E: AsyncWrite + Unpin,
    C: Future<Output = ()>,
{
    tokio::pin!(cancel);
    let (answer, sender) = first_or_cancel((reader, writer), cancel.as_mut()).await?;
    let header = match answer {
        First::Finished(frame) => {
            let result = decoded(decode_call_outcome(frame.payload()))?;
            return Ok(Called {
                header: None,
                result,
                delivery_interrupted: false,
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
    let (lost, stop) = (Notify::new(), CancellationToken::new());
    let mut outbound = Box::pin(outbound(
        reader,
        Sinks::new((output, error), true, (&lost, &stop)),
    ));
    // A failed local read is kept even when the outcome arrives before the cancel is sent.
    let mut unread = None;
    // Once cancelled, nothing but the rest of that `cancel` is sent; the outcome still follows.
    let (answer, unwritten, delivery_interrupted) = match sender {
        Sender::Cancelled(rest) => {
            stop.cancel();
            drained(outbound.as_mut(), rest).await.unwrap_or((
                Err(ClientError::Stream(ServiceError::OutcomeUnknown)),
                None,
                false,
            ))
        }
        Sender::Open(writer) => {
            let control = ProcessControl {
                resizes: None,
                cancel,
            };
            let mut inbound = Box::pin(inbound(
                writer,
                (input, control),
                (&lost, &stop, &mut unread),
            ));
            tokio::select! {
                answer = &mut outbound => answer,
                () = &mut inbound => drained(outbound.as_mut(), None)
                    .await
                    .unwrap_or((Err(ClientError::Stream(ServiceError::OutcomeUnknown)), None, false)),
            }
        }
    };
    Ok(Called {
        header: Some(header),
        result: settled((answer, unwritten), unread)?,
        delivery_interrupted,
    })
}

/// Service → caller until the outcome; lost local output asks the inbound side to cancel.
/// Returns the answer, the first local write that failed, and whether delivery was interrupted.
async fn outbound<R, O, E>(
    reader: &mut FrameReader<R>,
    mut sinks: Sinks<'_, O, E>,
) -> (Result<Vec<u8>, ClientError>, Option<std::io::Error>, bool)
where
    R: AsyncRead + Unpin,
    O: AsyncWrite + Unpin,
    E: AsyncWrite + Unpin,
{
    let answer = async {
        loop {
            let frame = next(reader).await?;
            match frame.kind() {
                FrameKind::Outcome => return decoded(decode_call_outcome(frame.payload())),
                // Control messages are the service's own; this generic caller has no use for
                // them.
                FrameKind::Control => {}
                _ => sinks.take(&frame).await?,
            }
        }
    }
    .await;
    (answer, sinks.unwritten, sinks.discarding)
}
