//! Caller-to-supervisor stdin forwarding for an attachment relay.

use ma2a_core::service::{Channel as DataChannel, FrameKind, ServiceError};
use ma2a_services::{Frame, FrameReader, FrameWriter};
use tokio::io::{AsyncRead, AsyncWrite};

use super::super::attach::CALLER_STALL;
use super::DETACH_GRACE;

/// How the caller's side of a relay ended.
pub(super) enum Upstream {
    /// The supervisor took the detach.
    Detached,
    /// The caller detached, but the supervisor's connection was closed and could not take it;
    /// what the supervisor sent before still follows.
    Closed,
    /// The supervisor did not take input or the detach in time.
    Stalled,
}

enum Forward {
    Done(Result<(), ServiceError>),
    Stop,
}

pub(super) async fn relay_upstream<R, SW>(
    (reader, mut to_supervisor): (&mut FrameReader<R>, FrameWriter<SW>),
    stdin: bool,
    bytes_in: &mut u64,
) -> Upstream
where
    R: AsyncRead + Unpin,
    SW: AsyncWrite + Unpin,
{
    // Returning detaches: the supervisor then reports where the Process is. Input the Process
    // does not take within the stall bound detaches too, so a caller's detach is never stuck
    // behind it. A supervisor that closed its connection, for example once the child ended,
    // takes nothing more: later input is dropped, its output is still delivered, and the
    // caller's detach is still seen.
    let mut open = true;
    let mut input_ended = false;
    let mut ended: Option<Frame> = None;
    let mut ahead: Option<Frame> = None;
    while let Some(frame) = next_frame(reader, &mut ended, &mut ahead).await {
        let forwarded = match (frame.kind(), frame.data(), frame.end()) {
            (FrameKind::Data, Ok((DataChannel::Input, _)), _)
            | (FrameKind::End, _, Ok(DataChannel::Input))
                if stdin && !open =>
            {
                continue;
            }
            (FrameKind::Data, Ok((DataChannel::Input, bytes)), _) if stdin && !input_ended => {
                *bytes_in += u64::try_from(bytes.len()).unwrap_or(u64::MAX);
                let write = to_supervisor.data(DataChannel::Input, bytes);
                forward_watching(reader, (&mut ended, &mut ahead, input_ended), write).await
            }
            (FrameKind::End, _, Ok(DataChannel::Input)) if stdin && !input_ended => {
                input_ended = true;
                let write = to_supervisor.end(DataChannel::Input);
                forward_watching(reader, (&mut ended, &mut ahead, input_ended), write).await
            }
            _ => break,
        };
        match forwarded {
            Forward::Done(Ok(())) => {}
            Forward::Done(Err(ServiceError::Cancelled)) => open = false,
            // A write cut short by the stall bound or caller cancel/EOF left a torn frame: no
            // detach follows.
            Forward::Stop => return Upstream::Stalled,
            Forward::Done(Err(_)) => break,
        }
    }
    if !open {
        return Upstream::Closed;
    }
    let cancel = to_supervisor.frame(FrameKind::Cancel, &[]);
    match tokio::time::timeout(DETACH_GRACE, cancel).await {
        Ok(Ok(())) => Upstream::Detached,
        Ok(Err(_)) => Upstream::Closed,
        Err(_) => Upstream::Stalled,
    }
}

async fn next_frame<R: AsyncRead + Unpin>(
    reader: &mut FrameReader<R>,
    ended: &mut Option<Frame>,
    ahead: &mut Option<Frame>,
) -> Option<Frame> {
    if let Some(frame) = ended.take().or_else(|| ahead.take()) {
        return Some(frame);
    }
    reader.next().await.ok().flatten()
}

async fn forward_watching<R, F>(
    reader: &mut FrameReader<R>,
    (ended, ahead, input_ended): (&mut Option<Frame>, &mut Option<Frame>, bool),
    write: F,
) -> Forward
where
    R: AsyncRead + Unpin,
    F: Future<Output = Result<(), ServiceError>>,
{
    let deadline = tokio::time::sleep(CALLER_STALL);
    tokio::pin!(write, deadline);
    loop {
        tokio::select! {
            biased;
            written = &mut write => return Forward::Done(written),
            // Keep watching for `cancel`, EOF, or `end` while the supervisor pipe is full so a
            // caller that detaches or disappears releases the attachment at once.
            next = reader.next(), if ahead.is_none() => match next {
                Ok(Some(next)) if next.kind() == FrameKind::Cancel => return Forward::Stop,
                Ok(Some(next))
                    if !input_ended && ended.is_none() && next.end() == Ok(DataChannel::Input) =>
                {
                    *ended = Some(next);
                }
                Ok(Some(_)) if input_ended || ended.is_some() => return Forward::Stop,
                Ok(Some(next))
                    if next.kind() == FrameKind::Data
                        && matches!(next.data(), Ok((DataChannel::Input, _))) =>
                {
                    *ahead = Some(next);
                }
                Ok(None | Some(_)) | Err(_) => return Forward::Stop,
            },
            () = &mut deadline => return Forward::Stop,
        }
    }
}
