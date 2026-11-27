//! Stdin writes, relayed to the authenticated supervisor.
//!
//! A write ends when the caller ends its input, and also when the supervisor answers first,
//! for example because the Process ended: a caller that keeps its input open must not hold
//! the operation open past the Process. Input the Process does not take within the stall
//! bound fails the write, also the rest it still has to take once the input ended, and
//! meanwhile a caller can still cancel, so neither holds the operation for as long as the
//! Process runs.

use ma2a_core::service::{
    Channel as DataChannel, FrameKind, ProcessAccept, ProcessId, ProcessReply, ServiceError,
};
use ma2a_services::{
    Frame, FrameReader, FrameWriter, OperationEnd, OperationReport,
    reply::client_cancelled,
    supervisor::{SupervisorRequest, protocol::decode_written},
};
use tokio::io::{AsyncRead, AsyncWrite};

use super::{
    ProcessManager,
    attach::{CALLER_STALL, Caller, Scope, lost},
    host::{finish, refuse, write_accept},
};

impl ProcessManager {
    /// Writes the caller's channel 0 to the Process stdin; `close` ends it afterwards.
    pub(super) async fn stdin<R, W>(
        &self,
        ((viewer, stop), (id, close)): (Scope<'_>, (ProcessId, bool)),
        (reader, writer): Caller<'_, R, W>,
    ) -> OperationReport
    where
        R: AsyncRead + Unpin + Send,
        W: AsyncWrite + Unpin + Send,
    {
        let request = SupervisorRequest::Stdin(close);
        let opened = tokio::select! {
            biased;
            // Withdrawn while opening, as once running: this Runtime no longer serves it.
            () = stop.cancelled() => Err(ServiceError::Unavailable),
            opened = self.open_live((viewer, id), request) => opened,
        };
        let (_, (mut from_supervisor, mut to_supervisor), _) = match opened {
            Ok(opened) => opened,
            Err(error) => return refuse(writer, error).await,
        };
        let Ok(accept) = ProcessAccept::Admitted.encode() else {
            return refuse(writer, ServiceError::Internal).await;
        };
        if let Err(error) = write_accept(writer, &accept).await {
            return OperationReport::new(OperationEnd::Failed(error));
        }
        let written = tokio::select! {
            biased;
            // The stream host reports a withdrawn authorization as such; the wire says only
            // that this Runtime no longer serves the operation.
            () = stop.cancelled() => Err(ServiceError::Unavailable),
            written = relay_stdin((&mut from_supervisor, &mut to_supervisor), reader) => written,
        };
        // Release the supervisor stdin lease before writing the terminal frame to the peer.
        drop((from_supervisor, to_supervisor));
        match written {
            Ok((bytes, closed)) => finish(writer, Ok(ProcessReply::Written(bytes, closed)))
                .await
                .with_bytes(bytes, 0),
            Err(error) => finish(writer, Err(error)).await,
        }
    }
}

/// Relays caller stdin frames to one supervisor channel until `end`, early supervisor answer,
/// caller cancellation, or stall.
pub(super) async fn relay_stdin<SR, SW, R>(
    (from_supervisor, to_supervisor): (&mut FrameReader<SR>, &mut FrameWriter<SW>),
    reader: &mut FrameReader<R>,
) -> Result<(u64, bool), ServiceError>
where
    SR: AsyncRead + Unpin,
    SW: AsyncWrite + Unpin,
    R: AsyncRead + Unpin,
{
    // The supervisor's one answer, read once whether it comes early or after `end`.
    let answer = from_supervisor.expect();
    tokio::pin!(answer);
    let mut ended: Option<Frame> = None;
    let mut ahead: Option<Frame> = None;
    let early = loop {
        let frame = if let Some(frame) = ended.take().or_else(|| ahead.take()) {
            frame
        } else {
            tokio::select! {
                frame = reader.next() => frame?.ok_or(ServiceError::Cancelled)?,
                answer = &mut answer => break Some(answer.map_err(lost)?),
            }
        };
        match frame.kind() {
            FrameKind::Data => {
                let (DataChannel::Input, bytes) = frame.data()? else {
                    return Err(ServiceError::InvalidInput);
                };
                let write = to_supervisor.data(DataChannel::Input, bytes);
                let deadline = tokio::time::sleep(CALLER_STALL);
                tokio::pin!(write, deadline);
                let early_answer = loop {
                    tokio::select! {
                        biased;
                        answer = &mut answer => break Some(answer.map_err(lost)?),
                        written = &mut write => {
                            written.map_err(lost)?;
                            break None;
                        }
                        // Keep watching for `cancel`, EOF, or `end` while the supervisor pipe is
                        // full so a cancelled caller releases its lease at once.
                        next = reader.next(), if ahead.is_none() => match next? {
                            None => return Err(ServiceError::Cancelled),
                            Some(next) if next.kind() == FrameKind::Cancel => {
                                return Err(ServiceError::Cancelled);
                            }
                            Some(next)
                                if ended.is_none() && next.end() == Ok(DataChannel::Input) =>
                            {
                                ended = Some(next);
                            }
                            Some(_) if ended.is_some() => return Err(ServiceError::InvalidInput),
                            Some(next)
                                if next.kind() == FrameKind::Data
                                    && next.data()?.0 == DataChannel::Input =>
                            {
                                ahead = Some(next);
                            }
                            Some(_) => return Err(ServiceError::InvalidInput),
                        },
                        () = &mut deadline => return Err(ServiceError::TimedOut),
                    }
                };
                if let Some(outcome) = early_answer {
                    break Some(outcome);
                }
            }
            FrameKind::End if frame.end()? == DataChannel::Input => break None,
            FrameKind::Cancel => return Err(ServiceError::Cancelled),
            _ => return Err(ServiceError::InvalidInput),
        }
    };
    let outcome = if let Some(outcome) = early {
        outcome
    } else {
        // The rest of the input has as long to be taken as any write: `end` and the supervisor's
        // answer behind it share one stall bound.
        let ended = async {
            let end = async {
                to_supervisor.end(DataChannel::Input).await.map_err(lost)?;
                std::future::pending().await
            };
            match tokio::time::timeout(CALLER_STALL, end).await {
                Ok(Err(error)) => error,
                Ok(Ok(())) | Err(_) => ServiceError::TimedOut,
            }
        };
        tokio::select! {
            biased;
            answer = &mut answer => answer.map_err(lost)?,
            error = ended => return Err(error),
            error = client_cancelled(reader) => return Err(error),
        }
    };
    match outcome.kind() {
        FrameKind::Outcome => decode_written(outcome.payload()),
        FrameKind::Reject => Err(outcome.rejection()?),
        _ => Err(ServiceError::Unavailable),
    }
}
