//! Attachments, relayed to the authenticated supervisor.
//!
//! The supervisor enforces the single stdin writer; this Runtime bounds attached readers
//! per Process. Detaching never affects the Process: the caller's `cancel` or loss only
//! ends this relay, and the outcome reports where the reader was.

use ma2a_core::{
    EndpointId,
    service::{
        EndReason, FrameKind, LogFrom, LogWindow, ProcessAccept, ProcessId, ProcessReply,
        ProcessState, ServiceError,
    },
};
use ma2a_services::{
    FrameReader, FrameWriter, OperationEnd, OperationReport,
    supervisor::{Channel, SupervisorRequest, protocol::decode_reading},
};
use ma2a_store::{Committed, ProcessRecord};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::sync::CancellationToken;

use super::{
    ProcessManager, credentials,
    host::{finish, refuse, visible, write_accept},
    relay::relay,
};

/// Most readers attached to one Process at a time.
const MAX_READERS: usize = 8;

pub(super) type Caller<'a, R, W> = (&'a mut FrameReader<R>, &'a mut FrameWriter<W>);
/// Longest wait for a live Process's supervisor to connect and answer an opened request.
pub(super) const SUPERVISOR_ANSWER: std::time::Duration = std::time::Duration::from_secs(10);
/// Longest wait for room toward a caller, so one that stops reading frees its reader slot and
/// stdin lease.
pub(super) const CALLER_STALL: std::time::Duration = std::time::Duration::from_secs(30);

/// Maps a failed supervisor connection, lost or answering malformed frames, to `unavailable`:
/// it is this Runtime's failure, and never a cancel by the caller.
pub(super) const fn lost(_: ServiceError) -> ServiceError {
    ServiceError::Unavailable
}

/// Who asks (`None` is the local operator) and what ends the operation early.
pub(super) type Scope<'a> = (Option<EndpointId>, &'a CancellationToken);

/// Holds one reader slot of a Process until dropped.
struct ReaderSlot(ProcessManager, ProcessId);

impl Drop for ReaderSlot {
    fn drop(&mut self) {
        let mut coordination = self.0.coordination();
        if let Some(count) = coordination.readers.get_mut(&self.1) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                coordination.readers.remove(&self.1);
            }
        }
    }
}

impl ProcessManager {
    fn reader_slot(&self, id: ProcessId) -> Option<ReaderSlot> {
        let mut coordination = self.coordination();
        let count = coordination.readers.entry(id).or_insert(0);
        if *count >= MAX_READERS {
            return None;
        }
        *count += 1;
        drop(coordination);
        Some(ReaderSlot(self.clone(), id))
    }

    /// Opens one request to the supervisor of a live Process and reads its first answer.
    pub(super) async fn open_live(
        &self,
        (viewer, id): (Option<EndpointId>, ProcessId),
        request: SupervisorRequest,
    ) -> Result<(ProcessRecord, Channel, Vec<u8>), ServiceError> {
        let record = visible(self.record(id).await?, viewer)?;
        let credentials = credentials(&record)
            .filter(|_| matches!(record.state, ProcessState::Starting | ProcessState::Running))
            .ok_or(ServiceError::NotRunning)?;
        // A supervisor that cannot be reached or proven is this Runtime's problem, never the
        // caller's authorization; the follower concludes a lost Process on its own.
        // A supervisor that connects but never answers holds nobody past this.
        let opened = tokio::time::timeout(SUPERVISOR_ANSWER, async {
            let (mut reader, mut writer) = self
                .address(id)
                .connect(&credentials)
                .await
                .map_err(|_| ServiceError::Unavailable)?;
            writer
                .frame(FrameKind::Open, &request.to_bytes())
                .await
                .map_err(lost)?;
            let frame = reader.expect().await.map_err(lost)?;
            Ok::<_, ServiceError>((reader, writer, frame))
        })
        .await
        .map_err(|_| ServiceError::Unavailable)?;
        let (reader, writer, frame) = opened?;
        match frame.kind() {
            FrameKind::Accept => Ok((record, (reader, writer), frame.into_payload())),
            FrameKind::Reject => Err(frame.rejection()?),
            _ => Err(ServiceError::Unavailable),
        }
    }

    /// Follows the output of a live Process; with `stdin`, also relays channel 0.
    pub(super) async fn attach<R, W>(
        &self,
        ((viewer, stop), (id, from, stdin)): (Scope<'_>, (ProcessId, LogFrom, bool)),
        (reader, writer): Caller<'_, R, W>,
    ) -> OperationReport
    where
        R: AsyncRead + Unpin + Send,
        W: AsyncWrite + Unpin + Send,
    {
        // Ownership first, so a stranger never learns from the reader limit that it exists.
        if let Err(error) = self
            .record(id)
            .await
            .and_then(|record| visible(record, viewer))
        {
            return refuse(writer, error).await;
        }
        let Some(_slot) = self.reader_slot(id) else {
            return refuse(writer, ServiceError::ConcurrencyExceeded).await;
        };
        let request = SupervisorRequest::Attach(from, stdin);
        let opened = tokio::select! {
            biased;
            // Withdrawn while opening, as once running: this Runtime no longer serves it.
            () = stop.cancelled() => Err(ServiceError::Unavailable),
            opened = self.open_live((viewer, id), request) => opened,
        };
        let (record, supervisor, accepted) = match opened {
            Ok(opened) => opened,
            Err(error) => return refuse(writer, error).await,
        };
        let Ok(((start, first, end), truncated)) = decode_reading(&accepted) else {
            return refuse(writer, ServiceError::Unavailable).await;
        };
        let accept = ProcessAccept::Reading {
            state: record.state,
            start,
            window: LogWindow::new(first, end),
            truncated,
        };
        let Ok(accept) = accept.encode() else {
            return refuse(writer, ServiceError::Internal).await;
        };
        if let Err(error) = write_accept(writer, &accept).await {
            return OperationReport::new(OperationEnd::Failed(error));
        }
        let relayed = relay(supervisor, (reader, writer), ((stdin, start), stop)).await;
        let (reported, cursor, bytes) = match relayed {
            Ok(relayed) => relayed,
            Err(error) => return finish(writer, Err(error)).await,
        };
        let (record, reason) = match self.apply(id, reported).await {
            Ok(applied) if reported.is_final() => (applied.into_value(), EndReason::Completed),
            // An end that could not be recorded is not reported over a summary still live.
            Err(_) if reported.is_final() => {
                return finish(writer, Err(ServiceError::Unavailable))
                    .await
                    .with_bytes(bytes.0, bytes.1);
            }
            applied => (
                applied.map_or(record, Committed::into_value),
                EndReason::Cancelled,
            ),
        };
        let detached = ProcessReply::Detached(reason, record.summary(), cursor);
        finish(writer, Ok(detached))
            .await
            .with_bytes(bytes.0, bytes.1)
    }
}
