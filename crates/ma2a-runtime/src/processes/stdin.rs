//! Stdin writes, relayed to the authenticated supervisor.
//!
//! A write ends when the caller ends its input, and also when the supervisor answers first,
//! for example because the Process ended: a caller that keeps its input open must not hold
//! the operation open past the Process.

use ma2a_core::service::{
    Channel as DataChannel, FrameKind, ProcessAccept, ProcessId, ProcessReply, ServiceError,
};
use ma2a_services::{
    OperationEnd, OperationReport,
    supervisor::{SupervisorRequest, protocol::decode_written},
};
use tokio::io::{AsyncRead, AsyncWrite};

use super::{
    ProcessManager,
    attach::{Caller, Scope},
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
            () = stop.cancelled() => Err(ServiceError::Cancelled),
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
        let written = async {
            // The supervisor's one answer, read once whether it comes early or after `end`.
            let answer = from_supervisor.expect();
            tokio::pin!(answer);
            let early = loop {
                let frame = tokio::select! {
                    frame = reader.next() => frame?.ok_or(ServiceError::Cancelled)?,
                    answer = &mut answer => break Some(answer?),
                };
                match frame.kind() {
                    FrameKind::Data => {
                        let (DataChannel::Input, bytes) = frame.data()? else {
                            return Err(ServiceError::InvalidInput);
                        };
                        to_supervisor.data(DataChannel::Input, bytes).await?;
                    }
                    FrameKind::End if frame.end()? == DataChannel::Input => break None,
                    FrameKind::Cancel => return Err(ServiceError::Cancelled),
                    _ => return Err(ServiceError::InvalidInput),
                }
            };
            let outcome = if let Some(outcome) = early {
                outcome
            } else {
                to_supervisor.end(DataChannel::Input).await?;
                answer.await?
            };
            match outcome.kind() {
                FrameKind::Outcome => decode_written(outcome.payload()),
                FrameKind::Reject => Err(outcome.rejection()?),
                _ => Err(ServiceError::Unavailable),
            }
        };
        let written = tokio::select! {
            biased;
            // The stream host reports a withdrawn authorization as such; the wire says only
            // that this Runtime no longer serves the operation.
            () = stop.cancelled() => Err(ServiceError::Unavailable),
            written = written => written,
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
