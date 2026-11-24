//! Caller side of Agent v1: content-free operations and `watch`.
//!
//! `artifact` answers with Transfer v1 download frames, so its caller uses
//! [`super::transfer::download_accepted`] and [`super::transfer::download_content`].

use ma2a_core::service::{AgentAccept, AgentEvent, AgentReply, Channel, FrameKind, ServiceError};
use tokio::io::AsyncRead;

use super::{ClientError, First, decoded, first, next, unexpected};
use crate::{Frame, FrameReader};

/// Reads one content-free answer: `agents`, `submit`, `send`, `get`, `tasks`, or `cancel`.
///
/// Returns the accept, or `None` when the outcome was answered from caller-side replay.
///
/// # Errors
/// Returns the refusal, the failed outcome, or the stream or protocol failure.
pub async fn call<R: AsyncRead + Unpin>(
    reader: &mut FrameReader<R>,
) -> Result<(Option<AgentAccept>, AgentReply), ClientError> {
    let accept = match first(reader).await? {
        First::Finished(frame) => return Ok((None, reply(&frame)?)),
        First::Accepted(frame) => accepted(&frame)?,
    };
    let frame = next(reader).await?;
    if frame.kind() != FrameKind::Outcome {
        return Err(unexpected(&frame));
    }
    Ok((Some(accept), reply(&frame)?))
}

/// Follows one `watch`, handing every event to `on_event` in order, until the outcome:
/// the task's summary once it ended.
///
/// A target that accepts the watch as anything but `Watching`, sends an event at or before
/// one it already sent (or before the `next` it announced), or ends the watch with anything
/// but an ended task's summary is malformed: a caller would otherwise replay events or take
/// a live task for an ended one.
///
/// # Errors
/// Returns the refusal, the failed outcome, or the stream or protocol failure.
pub async fn watch<R, F>(
    reader: &mut FrameReader<R>,
    mut on_event: F,
) -> Result<(AgentAccept, AgentReply), ClientError>
where
    R: AsyncRead + Unpin,
    F: FnMut(AgentEvent),
{
    let malformed = || ClientError::Stream(ServiceError::InvalidInput);
    let First::Accepted(frame) = first(reader).await? else {
        return Err(malformed());
    };
    let accept = accepted(&frame)?;
    let AgentAccept::Watching(_, next_seq) = accept else {
        return Err(malformed());
    };
    // The lowest sequence number the next event may carry.
    let mut floor = u64::from(next_seq);
    loop {
        let frame = next(reader).await?;
        match frame.kind() {
            FrameKind::Data => match frame.data().map_err(ClientError::Stream)? {
                (Channel::Output, bytes) => {
                    let event = AgentEvent::from_bytes(bytes)
                        .map_err(|error| ClientError::Stream(ServiceError::from_protocol(error)))?;
                    if u64::from(event.seq) < floor {
                        return Err(malformed());
                    }
                    floor = u64::from(event.seq) + 1;
                    on_event(event);
                }
                _ => return Err(malformed()),
            },
            FrameKind::Outcome => {
                return match reply(&frame)? {
                    AgentReply::Summary(summary) if summary.state.is_terminal() => {
                        Ok((accept, AgentReply::Summary(summary)))
                    }
                    _ => Err(malformed()),
                };
            }
            _ => return Err(unexpected(&frame)),
        }
    }
}

fn accepted(frame: &Frame) -> Result<AgentAccept, ClientError> {
    AgentAccept::decode(frame.payload())
        .map_err(|error| ClientError::Stream(ServiceError::from_protocol(error)))
}

fn reply(frame: &Frame) -> Result<AgentReply, ClientError> {
    decoded(AgentReply::decode_outcome(frame.payload()))
}
