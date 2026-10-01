//! Attachments and stdin writes, relayed to the authenticated supervisor.
//!
//! The supervisor enforces the single stdin writer; this Runtime bounds attached readers
//! per Process. Detaching never affects the Process: the caller's `cancel` or loss only
//! ends this relay, and the outcome reports where the reader was.

use ma2a_core::{
    EndpointId,
    service::{
        Channel as DataChannel, EndReason, FrameKind, LogFrom, LogWindow, ProcessAccept, ProcessId,
        ProcessReply, ProcessState, ServiceError, decode_skipped,
    },
};
use ma2a_services::{
    FrameReader, FrameWriter, OperationEnd, OperationReport,
    supervisor::{
        Channel, SupervisorRequest, SupervisorStatus,
        protocol::{decode_reading, decode_written},
    },
};
use ma2a_store::ProcessRecord;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::sync::CancellationToken;

use super::{
    ProcessManager, credentials,
    host::{finish, refuse, visible},
};

/// Most readers attached to one Process at a time.
const MAX_READERS: usize = 8;

type Caller<'a, R, W> = (&'a mut FrameReader<R>, &'a mut FrameWriter<W>);
/// Who asks (`None` is the local operator) and what ends the operation early.
type Scope<'a> = (Option<EndpointId>, &'a CancellationToken);

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
        let (mut reader, mut writer) = self
            .address(id)
            .connect(&credentials)
            .await
            .map_err(|_| ServiceError::Unavailable)?;
        writer.frame(FrameKind::Open, &request.to_bytes()).await?;
        let frame = reader.expect().await?;
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
        let (record, supervisor, accepted) = match self.open_live((viewer, id), request).await {
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
        if writer.frame(FrameKind::Accept, &accept).await.is_err() {
            return OperationReport::new(OperationEnd::Failed(ServiceError::Cancelled));
        }
        let relayed = relay(supervisor, (reader, writer), ((stdin, start), stop)).await;
        let (reported, cursor, bytes) = match relayed {
            Ok(relayed) => relayed,
            Err(error) => return finish(writer, Err(error)).await,
        };
        let record = self.apply(id, reported).await.unwrap_or(record);
        let reason = if reported.is_final() {
            EndReason::Completed
        } else {
            EndReason::Cancelled
        };
        let detached = ProcessReply::Detached(reason, record.summary(), cursor);
        finish(writer, Ok(detached))
            .await
            .with_bytes(bytes.0, bytes.1)
    }

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
        let (_, (mut from_supervisor, mut to_supervisor), _) =
            match self.open_live((viewer, id), request).await {
                Ok(opened) => opened,
                Err(error) => return refuse(writer, error).await,
            };
        let Ok(accept) = ProcessAccept::Admitted.encode() else {
            return refuse(writer, ServiceError::Internal).await;
        };
        if writer.frame(FrameKind::Accept, &accept).await.is_err() {
            return OperationReport::new(OperationEnd::Failed(ServiceError::Cancelled));
        }
        let written = async {
            loop {
                let frame = reader.next().await?.ok_or(ServiceError::Cancelled)?;
                match frame.kind() {
                    FrameKind::Data => {
                        let (DataChannel::Input, bytes) = frame.data()? else {
                            return Err(ServiceError::InvalidInput);
                        };
                        to_supervisor.data(DataChannel::Input, bytes).await?;
                    }
                    FrameKind::End if frame.end()? == DataChannel::Input => break,
                    FrameKind::Cancel => return Err(ServiceError::Cancelled),
                    _ => return Err(ServiceError::InvalidInput),
                }
            }
            to_supervisor.end(DataChannel::Input).await?;
            let outcome = from_supervisor.expect().await?;
            match outcome.kind() {
                FrameKind::Outcome => decode_written(outcome.payload()),
                FrameKind::Reject => Err(outcome.rejection()?),
                _ => Err(ServiceError::Unavailable),
            }
        };
        let written = tokio::select! {
            written = written => written,
            // The stream host reports a withdrawn authorization as such; the wire says only
            // that this Runtime no longer serves the operation.
            () = stop.cancelled() => Err(ServiceError::Unavailable),
        };
        match written {
            Ok((bytes, closed)) => finish(writer, Ok(ProcessReply::Written(bytes, closed)))
                .await
                .with_bytes(bytes, 0),
            Err(error) => finish(writer, Err(error)).await,
        }
    }
}

/// What a relay reports: the supervisor's last status, the cursor reached, bytes in/out.
type Relayed = (SupervisorStatus, u64, (u64, u64));

/// Relays one attachment until the supervisor's `outcome`.
async fn relay<R, W>(
    (mut from_supervisor, mut to_supervisor): Channel,
    (reader, writer): Caller<'_, R, W>,
    ((stdin, mut cursor), stop): ((bool, u64), &CancellationToken),
) -> Result<Relayed, ServiceError>
where
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send,
{
    let mut bytes_in = 0_u64;
    let mut bytes_out = 0_u64;
    let reported = {
        let downstream = async {
            loop {
                let frame = from_supervisor.expect().await?;
                match frame.kind() {
                    FrameKind::Data => {
                        let (channel, bytes) = frame.data()?;
                        writer.data(channel, bytes).await?;
                        let length = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
                        cursor += length;
                        bytes_out += length;
                    }
                    FrameKind::Control => {
                        cursor = decode_skipped(frame.payload())
                            .map_err(|_| ServiceError::Unavailable)?;
                        writer.frame(FrameKind::Control, frame.payload()).await?;
                    }
                    FrameKind::Outcome => {
                        return SupervisorStatus::from_bytes(frame.payload())
                            .map_err(|_| ServiceError::Unavailable);
                    }
                    _ => return Err(ServiceError::Unavailable),
                }
            }
        };
        let upstream = async {
            // Returning detaches: the supervisor then reports where the Process is.
            while let Ok(Some(frame)) = reader.next().await {
                let forwarded = match (frame.kind(), frame.data(), frame.end()) {
                    (FrameKind::Data, Ok((DataChannel::Input, bytes)), _) if stdin => {
                        bytes_in += u64::try_from(bytes.len()).unwrap_or(u64::MAX);
                        to_supervisor.data(DataChannel::Input, bytes).await
                    }
                    (FrameKind::End, _, Ok(DataChannel::Input)) if stdin => {
                        to_supervisor.end(DataChannel::Input).await
                    }
                    _ => break,
                };
                if forwarded.is_err() {
                    break;
                }
            }
            let _detached = to_supervisor.frame(FrameKind::Cancel, &[]).await;
        };
        tokio::pin!(downstream, upstream);
        let mut detached = false;
        loop {
            tokio::select! {
                biased;
                () = stop.cancelled() => break Err(ServiceError::Unavailable),
                reported = &mut downstream => break reported,
                () = &mut upstream, if !detached => detached = true,
            }
        }
    }?;
    Ok((reported, cursor, (bytes_in, bytes_out)))
}
