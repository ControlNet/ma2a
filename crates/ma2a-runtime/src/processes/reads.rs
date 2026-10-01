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

/// Where a read from `from` starts in `window`, and whether older output was dropped.
pub(super) fn resolve(from: LogFrom, window: LogWindow) -> Result<(u64, bool), ServiceError> {
    match from {
        LogFrom::Start => Ok((window.first_available, false)),
        LogFrom::Now => Ok((window.end, false)),
        LogFrom::Cursor(cursor) if cursor > window.end => Err(ServiceError::InvalidInput),
        LogFrom::Cursor(cursor) => Ok((
            cursor.max(window.first_available),
            cursor < window.first_available,
        )),
    }
}

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

    /// Returns the retained output window of one Process.
    pub(crate) async fn window(&self, id: ProcessId) -> LogWindow {
        let directory = log_directory(&self.directory(id));
        tokio::task::spawn_blocking(move || log::window(&directory))
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or_default()
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
        let window = self.window(id).await;
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
        (viewer, (id, from, max)): (Option<EndpointId>, (ProcessId, LogFrom, u32)),
        writer: &mut FrameWriter<W>,
    ) -> OperationReport {
        let record = match self
            .record(id)
            .await
            .and_then(|record| visible(record, viewer))
        {
            Ok(record) => record,
            Err(error) => return refuse(writer, error).await,
        };
        let window = self.window(id).await;
        let (start, truncated) = match resolve(from, window) {
            Ok(resolved) => resolved,
            Err(error) => return refuse(writer, error).await,
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
        if writer.frame(FrameKind::Accept, &accept).await.is_err() {
            return OperationReport::new(OperationEnd::Failed(ServiceError::Cancelled));
        }
        let budget = usize::try_from(max).unwrap_or(usize::MAX);
        let (next, sent) = match self.send_output((id, start, budget), writer).await {
            Ok(read) => read,
            Err(error) => return finish(writer, Err(error)).await,
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
        writer: &mut FrameWriter<W>,
    ) -> Result<(u64, u64), ServiceError> {
        let directory = log_directory(&self.directory(id));
        let mut sent = 0_u64;
        while budget > 0 {
            let path = directory.clone();
            let batch = budget.min(BATCH);
            let read = tokio::task::spawn_blocking(move || log::read(&path, cursor, batch))
                .await
                .map_err(|_| ServiceError::Internal)?;
            let read = match read {
                Ok(read) => read,
                // No output was ever written, or the Process was archived: nothing to send.
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    return Ok((cursor, sent));
                }
                Err(_) => return Err(ServiceError::Io),
            };
            if read.start > cursor {
                // Retention dropped output while this read was running.
                let skipped = encode_skipped(read.start).map_err(|_| ServiceError::Internal)?;
                writer.frame(FrameKind::Control, &skipped).await?;
            }
            if read.records.is_empty() {
                return Ok((read.start.max(cursor), sent));
            }
            for (channel, bytes) in &read.records {
                writer.data(*channel, bytes).await?;
                let length = bytes.len();
                budget = budget.saturating_sub(length);
                sent += u64::try_from(length).unwrap_or(u64::MAX);
            }
            cursor = read.next;
        }
        Ok((cursor, sent))
    }
}

#[cfg(test)]
mod tests {
    use ma2a_core::service::{LogFrom, LogWindow, ServiceError};

    use super::resolve;

    #[test]
    fn positions_resolve_inside_the_retained_window() {
        let window = LogWindow::new(100, 250);
        assert_eq!(resolve(LogFrom::Start, window), Ok((100, false)));
        assert_eq!(resolve(LogFrom::Now, window), Ok((250, false)));
        assert_eq!(resolve(LogFrom::Cursor(120), window), Ok((120, false)));
        assert_eq!(resolve(LogFrom::Cursor(250), window), Ok((250, false)));
        assert_eq!(resolve(LogFrom::Cursor(3), window), Ok((100, true)));
        assert_eq!(
            resolve(LogFrom::Cursor(251), window),
            Err(ServiceError::InvalidInput)
        );
    }
}
