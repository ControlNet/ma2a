//! Attachments, relayed to the authenticated supervisor.
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
    supervisor::{Channel, SupervisorRequest, SupervisorStatus, protocol::decode_reading},
};
use ma2a_store::ProcessRecord;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::sync::CancellationToken;

use super::{
    ProcessManager, credentials,
    host::{finish, refuse, visible, write_accept},
};

/// Most readers attached to one Process at a time.
const MAX_READERS: usize = 8;

pub(super) type Caller<'a, R, W> = (&'a mut FrameReader<R>, &'a mut FrameWriter<W>);
/// Longest wait for a live Process's supervisor to connect and answer an opened request.
pub(super) const SUPERVISOR_ANSWER: std::time::Duration = std::time::Duration::from_secs(10);
/// Longest wait for room toward a caller, so one that stops reading frees its reader slot and
/// stdin lease.
pub(super) const CALLER_STALL: std::time::Duration = std::time::Duration::from_secs(30);
/// Longest wait for the supervisor's report once the caller detached.
const DETACH_GRACE: std::time::Duration = std::time::Duration::from_secs(5);

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
            writer.frame(FrameKind::Open, &request.to_bytes()).await?;
            let frame = reader.expect().await?;
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
            () = stop.cancelled() => Err(ServiceError::Cancelled),
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
                        tokio::time::timeout(CALLER_STALL, writer.data(channel, bytes))
                            .await
                            .map_err(|_| ServiceError::TimedOut)??;
                        let length = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
                        cursor += length;
                        bytes_out += length;
                    }
                    FrameKind::Control => {
                        cursor = decode_skipped(frame.payload())
                            .map_err(|_| ServiceError::Unavailable)?;
                        tokio::time::timeout(
                            CALLER_STALL,
                            writer.frame(FrameKind::Control, frame.payload()),
                        )
                        .await
                        .map_err(|_| ServiceError::TimedOut)??;
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
            // Returning detaches: the supervisor then reports where the Process is. Input the
            // Process does not take within the stall bound detaches too, so a caller's detach
            // is never stuck behind it.
            while let Ok(Some(frame)) = reader.next().await {
                let forwarded = match (frame.kind(), frame.data(), frame.end()) {
                    (FrameKind::Data, Ok((DataChannel::Input, bytes)), _) if stdin => {
                        bytes_in += u64::try_from(bytes.len()).unwrap_or(u64::MAX);
                        let write = to_supervisor.data(DataChannel::Input, bytes);
                        tokio::time::timeout(CALLER_STALL, write).await
                    }
                    (FrameKind::End, _, Ok(DataChannel::Input)) if stdin => {
                        let write = to_supervisor.end(DataChannel::Input);
                        tokio::time::timeout(CALLER_STALL, write).await
                    }
                    _ => break,
                };
                if !matches!(forwarded, Ok(Ok(()))) {
                    break;
                }
            }
            let cancel = to_supervisor.frame(FrameKind::Cancel, &[]);
            matches!(tokio::time::timeout(DETACH_GRACE, cancel).await, Ok(Ok(())))
        };
        let grace = tokio::time::sleep(std::time::Duration::MAX);
        tokio::pin!(downstream, upstream, grace);
        let mut detached = false;
        loop {
            tokio::select! {
                biased;
                () = stop.cancelled() => break Err(ServiceError::Unavailable),
                reported = &mut downstream => break reported,
                delivered = &mut upstream, if !detached => {
                    // A detach the supervisor did not take has used up the grace already.
                    if !delivered {
                        break Err(ServiceError::Cancelled);
                    }
                    detached = true;
                    grace.as_mut().reset(tokio::time::Instant::now() + DETACH_GRACE);
                }
                () = &mut grace, if detached => break Err(ServiceError::Cancelled),
            }
        }
    }?;
    Ok((reported, cursor, (bytes_in, bytes_out)))
}
