//! Caller side of Agent v1: content-free operations and `watch`.
//!
//! `artifact` answers with Transfer v1 download frames, so its caller uses
//! [`super::transfer::download_accepted`] and [`super::transfer::download_content`].

use ma2a_core::service::{
    AgentAccept, AgentEvent, AgentEventBody, AgentId, AgentReply, AgentTaskId, Channel, FrameKind,
    ServiceError,
};
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

/// Follows one `watch` of `agent`'s `task` after the cursor `after`, handing every event to
/// `on_event` in order, until the outcome: the task's summary once it ended.
///
/// A target that accepts the watch as anything but `Watching` with `next = after + 1`
/// (saturating), sends an event at or before `after` or one it already sent, or ends the
/// watch with anything but the summary of that Agent's `task` once ended is malformed: a
/// caller would otherwise replay events, move its cursor backwards, or take a live task for
/// an ended one. So is a summary whose newest event is not the last one sent, or that last
/// event is not the `state` event of the end the summary reports, or, when none was sent, a
/// summary whose newest event is not in `1..=after`: the terminal event is never pruned, and
/// a caller would miss the task's end.
///
/// # Errors
/// Returns the refusal, the failed outcome, or the stream or protocol failure.
pub async fn watch<R, F>(
    reader: &mut FrameReader<R>,
    (agent, task, after): (&AgentId, AgentTaskId, u32),
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
    // A target announces the event after the cursor sent; any other is a stale answer.
    match accept {
        AgentAccept::Watching(_, next) if next == after.saturating_add(1) => {}
        _ => return Err(malformed()),
    }
    // The lowest sequence number the next event may carry, past `after` even at its maximum.
    let mut floor = u64::from(after) + 1;
    // The newest event sent so far, if any, and the end it reports when it is a `state` one.
    let mut last = None;
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
                    let ended = match event.body {
                        AgentEventBody::State(state, reason) => Some((state, reason)),
                        _ => None,
                    };
                    last = Some((event.seq, ended));
                    on_event(event);
                }
                _ => return Err(malformed()),
            },
            FrameKind::Outcome => {
                return match reply(&frame)? {
                    AgentReply::Summary(summary)
                        if summary.state.is_terminal()
                            && (&summary.agent, summary.id) == (agent, task)
                            && last.map_or_else(
                                || (1..=after).contains(&summary.last_seq),
                                |last| {
                                    last == (
                                        summary.last_seq,
                                        Some((summary.state, summary.reason)),
                                    )
                                },
                            ) =>
                    {
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
