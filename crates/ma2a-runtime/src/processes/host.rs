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
use ma2a_store::{Committed, ProcessArchive, ProcessRecord};
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
            let ended = ended.map(|record| ProcessReply::Summary(record.value().summary()));
            answer_effect(writer, ended).await
        }
        ProcessRequest::Delete(id) => {
            let deleted = manager.delete(viewer, id).await;
            answer_effect(writer, deleted.map(|_| ProcessReply::Deleted)).await
        }
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

/// Writes `accept` within [`FRAME_GRACE`]: `timed_out` when the caller did not take it in
/// time, `cancelled` when the stream failed.
pub(super) async fn write_accept<W: AsyncWrite + Unpin + Send>(
    writer: &mut FrameWriter<W>,
    payload: &[u8],
) -> Result<(), ServiceError> {
    match tokio::time::timeout(FRAME_GRACE, writer.frame(FrameKind::Accept, payload)).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(_)) => Err(ServiceError::Cancelled),
        Err(_) => Err(ServiceError::TimedOut),
    }
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
    // An operation that failed keeps its own failure even when its outcome could not be sent.
    match sent {
        Ok(Ok(())) => OperationReport::new(end),
        _ if end != OperationEnd::Completed => OperationReport::new(end),
        Ok(Err(_)) => OperationReport::new(OperationEnd::Failed(ServiceError::Cancelled)),
        Err(_) => OperationReport::new(OperationEnd::Failed(ServiceError::TimedOut)),
    }
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
    if let Err(error) = write_accept(writer, &accept).await {
        return OperationReport::new(OperationEnd::Failed(error));
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
    if let Err(error) = write_accept(writer, &accept).await {
        return OperationReport::new(OperationEnd::Failed(error));
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
        let frame = answered
            .map_err(|_| ServiceError::Unavailable)?
            .map_err(super::attach::lost)?;
        match frame.kind() {
            FrameKind::Outcome if frame.payload() == ACKNOWLEDGED => {
                Ok(ProcessReply::Summary(record.summary()))
            }
            FrameKind::Reject => Err(frame.rejection()?),
            _ => Err(ServiceError::Unavailable),
        }
    }

    /// Ends one Process: terminate, the grace period, then kill; answers once it ended, with
    /// the revision its end was committed at, or read at when it had ended already.
    pub(crate) async fn terminate(
        &self,
        (viewer, id): (Option<EndpointId>, ProcessId),
        (grace_ms, reason): (u32, ProcessReason),
    ) -> Result<Committed<ProcessRecord>, ServiceError> {
        let read = self.record_at(id).await?;
        let revision = read.revision();
        let record = visible(read.into_value(), viewer)?;
        if record.state.is_terminal() {
            return Ok(Committed::new(revision, record));
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
            .is_ok_and(|record| record.value().state.is_terminal())
        {
            self.coordination().terminating.remove(&id);
        }
        ended
    }

    /// Archives one terminal Process and removes its output; a tombstone keeps its binding.
    ///
    /// Returns the archived record at the revision its archive committed, which no later
    /// Store write or tombstone pruning changes.
    pub(crate) async fn delete(
        &self,
        viewer: Option<EndpointId>,
        id: ProcessId,
    ) -> Result<Committed<ProcessRecord>, ServiceError> {
        visible(self.record(id).await?, viewer)?;
        let at_ms = self.now_ms();
        let archived = self
            .store()
            .processes(move |repository| repository.archive_process(id, at_ms))
            .await?
            .map_err(|_| ServiceError::Unavailable)?;
        let revision = archived.revision();
        match archived.into_value() {
            ProcessArchive::Archived(record) => {
                self.remove_directory(id).await;
                Ok(Committed::new(revision, *record))
            }
            ProcessArchive::Active => Err(ServiceError::ProcessActive),
            ProcessArchive::Missing => Err(ServiceError::UnknownProcess),
        }
    }
}
