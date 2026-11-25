//! Reads: pages of Processes, one Process in full, and retained output.
//!
//! Output is read from the durable log directly, so it stays readable after the Process
//! and its supervisor ended, until the Process is archived.

use ma2a_core::{
    EndpointId,
    service::{
        FrameKind, LogFrom, LogWindow, MAX_PROCESS_PAGE, ProcessAccept, ProcessDetails, ProcessId,
        ProcessReply, ProcessStart, ServiceError, encode_skipped,
    },
};
use ma2a_services::{
    FrameWriter, OperationEnd, OperationReport,
    supervisor::{log, log_directory},
};
use ma2a_store::{ProcessFilter, ProcessRecord};
use tokio::io::AsyncWrite;

use super::{
    ProcessManager,
    host::{finish, refuse, visible},
};

/// Most content bytes read from the log for one batch of `data` frames.
const BATCH: usize = 262_144;

impl ProcessManager {
    /// Lists Processes in identifier order: a creator's own, or every one for the operator.
    pub(crate) async fn records(
        &self,
        viewer: Option<EndpointId>,
        (after, limit): (Option<ProcessId>, usize),
    ) -> Result<Vec<ProcessRecord>, ServiceError> {
        let filter = ProcessFilter {
            creator: viewer,
            after,
            limit: u32::try_from(limit).unwrap_or(u32::MAX),
            archived: viewer.is_none(),
        };
        Ok(self
            .store()
            .processes(move |repository| repository.processes(filter))
            .await?
            .map_err(|_| ServiceError::Unavailable)?
            .into_value())
    }

    pub(super) async fn list(
        &self,
        viewer: Option<EndpointId>,
        after: Option<ProcessId>,
    ) -> Result<ProcessReply, ServiceError> {
        let mut records = self.records(viewer, (after, MAX_PROCESS_PAGE + 1)).await?;
        let more = records.len() > MAX_PROCESS_PAGE;
        records.truncate(MAX_PROCESS_PAGE);
        let next = more
            .then(|| records.last().map(|record| record.id))
            .flatten();
        let page = records.iter().map(ProcessRecord::summary).collect();
        Ok(ProcessReply::Page(page, next))
    }

    /// Returns the retained output window of one Process: empty when it has no log or was
    /// archived, and `io` when a log exists but cannot be read.
    ///
    /// An archived Process has no output, even when a failed cleanup left its files behind.
    pub(crate) async fn window(&self, record: &ProcessRecord) -> Result<LogWindow, ServiceError> {
        if record.is_archived() {
            return Ok(LogWindow::default());
        }
        let directory = log_directory(&self.directory(record.id));
        tokio::task::spawn_blocking(move || log::window(&directory))
            .await
            .map_err(|_| ServiceError::Internal)?
            .map_err(|_| ServiceError::Io)
    }

    /// Returns one Process in full, with its start and retained output window.
    pub(crate) async fn details(
        &self,
        viewer: Option<EndpointId>,
        id: ProcessId,
    ) -> Result<(ProcessRecord, ProcessDetails), ServiceError> {
        let record = visible(self.record(id).await?, viewer)?;
        let start = record
            .start
            .as_deref()
            .map(ProcessStart::from_bytes)
            .transpose()
            .map_err(|_| ServiceError::Internal)?;
        let window = self.window(&record).await?;
        let details = ProcessDetails::new(record.summary(), start, window);
        Ok((record, details))
    }

    pub(super) async fn inspect(
        &self,
        viewer: Option<EndpointId>,
        id: ProcessId,
    ) -> Result<ProcessReply, ServiceError> {
        let (_, details) = self.details(viewer, id).await?;
        Ok(ProcessReply::Details(Box::new(details)))
    }

    /// Sends at most `max` retained bytes from `from`, then the cursor to continue from.
    pub(super) async fn logs<W: AsyncWrite + Unpin + Send>(
        &self,
        ((viewer, stop), (id, from, max)): (
            (Option<EndpointId>, &tokio_util::sync::CancellationToken),
            (ProcessId, LogFrom, u32),
        ),
        writer: &mut FrameWriter<W>,
    ) -> OperationReport {
        if stop.is_cancelled() {
            return OperationReport::new(OperationEnd::Failed(ServiceError::Unavailable));
        }
        let record = match self
            .record(id)
            .await
            .and_then(|record| visible(record, viewer))
        {
            Ok(record) => record,
            Err(error) => return refuse(writer, error).await,
        };
        let window = match self.window(&record).await {
            Ok(window) => window,
            Err(error) => return refuse(writer, error).await,
        };
        let Some((start, truncated)) = window.resolve(from) else {
            return refuse(writer, ServiceError::InvalidInput).await;
        };
        let accept = ProcessAccept::Reading {
            state: record.state,
            start,
            window,
            truncated,
        };
        let Ok(accept) = accept.encode() else {
            return refuse(writer, ServiceError::Internal).await;
        };
        if let Err(error) = super::host::write_accept(writer, &accept).await {
            return OperationReport::new(OperationEnd::Failed(error));
        }
        // Nothing is read from the files of an archived Process.
        let budget = if record.is_archived() {
            0
        } else {
            usize::try_from(max).unwrap_or(usize::MAX)
        };
        let (next, sent) = match self.send_output((id, start, budget), (stop, writer)).await {
            Ok(read) => read,
            Err((error, sent)) => return finish(writer, Err(error)).await.with_bytes(0, sent),
        };
        // Everything was read once the Process ended and no byte is left after `next`.
        let ended = record.state.is_terminal() && next >= window.end;
        let report = finish(writer, Ok(ProcessReply::Logs(next, ended))).await;
        report.with_bytes(0, sent)
    }

    /// Sends up to `budget` bytes from `cursor`; returns the cursor reached and bytes sent.
    async fn send_output<W: AsyncWrite + Unpin + Send>(
        &self,
        (id, mut cursor, mut budget): (ProcessId, u64, usize),
        (stop, writer): (&tokio_util::sync::CancellationToken, &mut FrameWriter<W>),
    ) -> Result<(u64, u64), (ServiceError, u64)> {
        const WRITE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);
        let directory = log_directory(&self.directory(id));
        let mut sent = 0_u64;
        while budget > 0 {
            if stop.is_cancelled() {
                return Err((ServiceError::Unavailable, sent));
            }
            let path = directory.clone();
            let batch = budget.min(BATCH);
            let read = tokio::task::spawn_blocking(move || log::read(&path, cursor, batch))
                .await
                .map_err(|_| (ServiceError::Internal, sent))?;
            let read = match read {
                Ok(read) => read,
                // No output was ever written, or the Process was archived: nothing to send.
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    return Ok((cursor, sent));
                }
                Err(_) => return Err((ServiceError::Io, sent)),
            };
            if read.start > cursor {
                // Retention dropped output while this read was running.
                let skipped =
                    encode_skipped(read.start).map_err(|_| (ServiceError::Internal, sent))?;
                tokio::select! {
                    biased;
                    () = stop.cancelled() => return Err((ServiceError::Unavailable, sent)),
                    wrote = tokio::time::timeout(WRITE_DEADLINE, writer.frame(FrameKind::Control, &skipped)) => {
                        match wrote {
                            Ok(Ok(())) => {}
                            Ok(Err(error)) => return Err((error, sent)),
                            Err(_) => return Err((ServiceError::TimedOut, sent)),
                        }
                    }
                }
            }
            if read.records.is_empty() {
                return Ok((read.start.max(cursor), sent));
            }
            for (channel, bytes) in &read.records {
                tokio::select! {
                    biased;
                    () = stop.cancelled() => return Err((ServiceError::Unavailable, sent)),
                    wrote = tokio::time::timeout(WRITE_DEADLINE, writer.data(*channel, bytes)) => {
                        match wrote {
                            Ok(Ok(())) => {}
                            Ok(Err(error)) => return Err((error, sent)),
                            Err(_) => return Err((ServiceError::TimedOut, sent)),
                        }
                    }
                }
                let length = bytes.len();
                budget = budget.saturating_sub(length);
                sent += u64::try_from(length).unwrap_or(u64::MAX);
            }
            cursor = read.next;
        }
        Ok((cursor, sent))
    }
}
