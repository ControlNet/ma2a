//! The Process family host: one operation per stream, owned Processes only.

use ma2a_core::{
    EndpointId, RequestId,
    service::{
        FrameKind, ProcessAccept, ProcessId, ProcessReason, ProcessReply, ProcessReplyOutcome,
        ProcessRequest, ProcessSignal, ProcessState, ServiceError,
    },
};
use ma2a_services::{
    FrameReader, FrameWriter, OperationEnd, OperationReport,
    supervisor::{SupervisorRequest, protocol::ACKNOWLEDGED},
};
use ma2a_store::{ProcessArchive, ProcessRecord};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::sync::CancellationToken;

use super::{ProcessManager, credentials, follow::request};

/// Serves one decoded Process operation for `peer`.
pub(crate) async fn serve<R, W>(
    manager: &ProcessManager,
    ((peer, request_id), request, stop): (
        (EndpointId, RequestId),
        ProcessRequest,
        &CancellationToken,
    ),
    (reader, writer): (&mut FrameReader<R>, &mut FrameWriter<W>),
) -> OperationReport
where
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send,
{
    let viewer = Some(peer);
    match request {
        ProcessRequest::Start(start) => manager.start(((peer, request_id), start), writer).await,
        ProcessRequest::List(after) => answer(writer, manager.list(viewer, after).await).await,
        ProcessRequest::Inspect(id) => answer(writer, manager.inspect(viewer, id).await).await,
        ProcessRequest::Logs(id, from, max) => {
            manager
                .logs(((viewer, stop), (id, from, max)), writer)
                .await
        }
        ProcessRequest::Attach(id, from, stdin) => {
            let operation = ((viewer, stop), (id, from, stdin));
            manager.attach(operation, (reader, writer)).await
        }
        ProcessRequest::Stdin(id, close) => {
            manager
                .stdin(((viewer, stop), (id, close)), (reader, writer))
                .await
        }
        ProcessRequest::Signal(id, signal) => {
            answer_effect(writer, manager.signal((viewer, id), signal).await).await
        }
        ProcessRequest::Terminate(id, grace_ms) => {
            let ended = manager
                .terminate((viewer, id), (grace_ms, ProcessReason::Requested))
                .await;
            let ended = ended.map(|record| ProcessReply::Summary(record.summary()));
            answer_effect(writer, ended).await
        }
        ProcessRequest::Delete(id) => answer_effect(writer, manager.delete(viewer, id).await).await,
    }
}

/// Refuses an operation before any effect.
pub(super) async fn refuse<W: AsyncWrite + Unpin + Send>(
    writer: &mut FrameWriter<W>,
    error: ServiceError,
) -> OperationReport {
    let _sent = tokio::time::timeout(FRAME_GRACE, writer.reject(error)).await;
    OperationReport::new(OperationEnd::Rejected(error))
}

/// Longest wait to write the frame that opens or ends an operation.
const FRAME_GRACE: std::time::Duration = std::time::Duration::from_secs(5);

/// Writes `accept` within [`FRAME_GRACE`]; `false` when it was not written in time.
pub(super) async fn write_accept<W: AsyncWrite + Unpin + Send>(
    writer: &mut FrameWriter<W>,
    payload: &[u8],
) -> bool {
    matches!(
        tokio::time::timeout(FRAME_GRACE, writer.frame(FrameKind::Accept, payload)).await,
        Ok(Ok(()))
    )
}

/// Sends the terminal outcome of an accepted operation within a bounded write grace.
pub(super) async fn finish<W: AsyncWrite + Unpin + Send>(
    writer: &mut FrameWriter<W>,
    outcome: ProcessReplyOutcome,
) -> OperationReport {
    let end = match &outcome {
        Ok(_) => OperationEnd::Completed,
        Err(error) => OperationEnd::Failed(*error),
    };
    let Ok(payload) = ProcessReply::encode_outcome(&outcome) else {
        return OperationReport::new(OperationEnd::Failed(ServiceError::Internal));
    };
    let sent = tokio::time::timeout(FRAME_GRACE, writer.frame(FrameKind::Outcome, &payload)).await;
    if !matches!(sent, Ok(Ok(()))) {
        return OperationReport::new(OperationEnd::Failed(ServiceError::Cancelled));
    }
    OperationReport::new(end)
}

/// Answers a content-free operation: its refusal, or `accept` and its outcome.
async fn answer<W: AsyncWrite + Unpin + Send>(
    writer: &mut FrameWriter<W>,
    reply: Result<ProcessReply, ServiceError>,
) -> OperationReport {
    let reply = match reply {
        Ok(reply) => reply,
        Err(error) => return refuse(writer, error).await,
    };
    let Ok(accept) = ProcessAccept::Admitted.encode() else {
        return refuse(writer, ServiceError::Internal).await;
    };
    if !write_accept(writer, &accept).await {
        return OperationReport::new(OperationEnd::Failed(ServiceError::Cancelled));
    }
    finish(writer, Ok(reply)).await
}

/// Answers an operation that may reach the supervisor or commit.
///
/// A refusal decided before any effect is a `reject`. Any later failure is a failed
/// `outcome`, because the signal, the terminate, or the archive may already have happened.
async fn answer_effect<W: AsyncWrite + Unpin + Send>(
    writer: &mut FrameWriter<W>,
    reply: Result<ProcessReply, ServiceError>,
) -> OperationReport {
    let error = match reply {
        Ok(reply) => return answer(writer, Ok(reply)).await,
        Err(
            error @ (ServiceError::UnknownProcess
            | ServiceError::NotRunning
            | ServiceError::ProcessActive),
        ) => return refuse(writer, error).await,
        // An unreachable or unproven supervisor is never the caller's authorization.
        Err(ServiceError::Unreachable | ServiceError::Unauthorized) => ServiceError::Unavailable,
        Err(error) => error,
    };
    let Ok(accept) = ProcessAccept::Admitted.encode() else {
        return refuse(writer, ServiceError::Internal).await;
    };
    if !write_accept(writer, &accept).await {
        return OperationReport::new(OperationEnd::Failed(ServiceError::Cancelled));
    }
    finish(writer, Err(error)).await
}

/// Returns the record `viewer` may see; the local operator (`None`) sees every Process.
pub(super) fn visible(
    record: Option<ProcessRecord>,
    viewer: Option<EndpointId>,
) -> Result<ProcessRecord, ServiceError> {
    match viewer {
        Some(peer) => super::owned(record, peer),
        None => record.ok_or(ServiceError::UnknownProcess),
    }
}

impl ProcessManager {
    pub(super) async fn signal(
        &self,
        (viewer, id): (Option<EndpointId>, ProcessId),
        signal: ProcessSignal,
    ) -> Result<ProcessReply, ServiceError> {
        let record = visible(self.record(id).await?, viewer)?;
        let credentials = credentials(&record)
            .filter(|_| record.state == ProcessState::Running)
            .ok_or(ServiceError::NotRunning)?;
        let (mut reader, mut writer) = self.address(id).connect(&credentials).await?;
        let answered = tokio::time::timeout(super::follow::ANSWER_MARGIN, async {
            writer
                .frame(
                    FrameKind::Open,
                    &SupervisorRequest::Signal(signal).to_bytes(),
                )
                .await?;
            reader.expect().await
        })
        .await;
        let frame = answered.map_err(|_| ServiceError::Unavailable)??;
        match frame.kind() {
            FrameKind::Outcome if frame.payload() == ACKNOWLEDGED => {
                Ok(ProcessReply::Summary(record.summary()))
            }
            FrameKind::Reject => Err(frame.rejection()?),
            _ => Err(ServiceError::Unavailable),
        }
    }

    /// Ends one Process: terminate, the grace period, then kill; answers once it ended.
    pub(crate) async fn terminate(
        &self,
        (viewer, id): (Option<EndpointId>, ProcessId),
        (grace_ms, reason): (u32, ProcessReason),
    ) -> Result<ProcessRecord, ServiceError> {
        let record = visible(self.record(id).await?, viewer)?;
        if record.state.is_terminal() {
            return Ok(record);
        }
        let credentials = credentials(&record)
            .filter(|_| record.state != ProcessState::Created)
            .ok_or(ServiceError::NotRunning)?;
        self.coordination().terminating.entry(id).or_insert(reason);
        let reported = match self.address(id).connect(&credentials).await {
            Ok(channel) => request(channel, SupervisorRequest::Terminate(grace_ms)).await,
            Err(error) => Err(error),
        };
        let ended = match reported {
            Ok(reported) => self.apply(id, reported).await,
            Err(ServiceError::Unreachable | ServiceError::Unauthorized) => {
                self.conclude(&record).await
            }
            Err(_) => Err(ServiceError::Unavailable),
        };
        // A termination that did not end the Process leaves nothing behind, so convergence
        // and later requests try again.
        if !ended
            .as_ref()
            .is_ok_and(|record| record.state.is_terminal())
        {
            self.coordination().terminating.remove(&id);
        }
        ended
    }

    /// Archives one terminal Process and removes its output; a tombstone keeps its binding.
    pub(crate) async fn delete(
        &self,
        viewer: Option<EndpointId>,
        id: ProcessId,
    ) -> Result<ProcessReply, ServiceError> {
        visible(self.record(id).await?, viewer)?;
        let at_ms = self.now_ms();
        let archived = self
            .store()
            .processes(move |repository| repository.archive_process(id, at_ms))
            .await?
            .map_err(|_| ServiceError::Unavailable)?
            .into_value();
        match archived {
            ProcessArchive::Archived(_) => {
                self.remove_directory(id).await;
                Ok(ProcessReply::Deleted)
            }
            ProcessArchive::Active => Err(ServiceError::ProcessActive),
            ProcessArchive::Missing => Err(ServiceError::UnknownProcess),
        }
    }
}
