//! Processes the local Endpoint creates for itself: Agent instances.
//!
//! They are ordinary Processes with the local Endpoint as creator, so no remote peer can
//! ever see or manage them, and withdrawal convergence never ends them. Their ended
//! records stay readable until the next start needs the room: it then archives the
//! oldest, so ended instances never block an Agent's restart.

use ma2a_core::{
    RequestId,
    service::{LogFrom, ProcessId, ProcessStart, ServiceError},
};
use ma2a_services::supervisor::{Channel, SupervisorRequest};
use ma2a_store::{ProcessCreation, ProcessRecord};

use super::ProcessManager;

impl ProcessManager {
    /// Starts one local Process under `request_id`, or returns the one it already made.
    pub(crate) async fn start_local(
        &self,
        request_id: RequestId,
        start: ProcessStart,
    ) -> Result<ProcessRecord, ServiceError> {
        if self.0.hosting.is_none() {
            return Err(ServiceError::Unavailable);
        }
        let binding = (self.0.local, request_id);
        let ((creation, archived), credentials) = self.create_local(binding, &start).await?;
        if let Some(id) = archived {
            self.remove_directory(id).await;
        }
        match creation {
            ProcessCreation::Created(_) => self.bring_up(credentials, start).await,
            ProcessCreation::Existing(record) => Ok(record),
            ProcessCreation::Conflict => Err(ServiceError::Conflict),
            ProcessCreation::LimitExceeded => Err(ServiceError::ConcurrencyExceeded),
        }
    }

    /// Attaches to a live local Process as its stdin writer, reading output from `from`.
    ///
    /// Returns the record, the proven supervisor channel, and the supervisor's accept.
    pub(crate) async fn attach_local(
        &self,
        id: ProcessId,
        from: LogFrom,
    ) -> Result<(ProcessRecord, Channel, Vec<u8>), ServiceError> {
        self.open_live((None, id), SupervisorRequest::AttachStrict(from))
            .await
    }

    /// Returns one Process record, whoever created it.
    pub(crate) async fn local_record(
        &self,
        id: ProcessId,
    ) -> Result<Option<ProcessRecord>, ServiceError> {
        self.record(id).await
    }

    /// Reads up to `max` content bytes from `cursor` of a local Process's durable log, or
    /// `None` when the Process has no log or was archived.
    pub(crate) async fn read_local_log(
        &self,
        record: &ProcessRecord,
        (cursor, max): (u64, usize),
    ) -> Result<Option<ma2a_services::supervisor::log::LogRead>, ServiceError> {
        if record.is_archived() {
            return Ok(None);
        }
        let directory = ma2a_services::supervisor::log_directory(&self.directory(record.id));
        let read = tokio::task::spawn_blocking(move || {
            ma2a_services::supervisor::log::read(&directory, cursor, max)
        })
        .await
        .map_err(|_| ServiceError::Internal)?;
        match read {
            Ok(read) => Ok(Some(read)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) if error.kind() == std::io::ErrorKind::NotADirectory => {
                Err(ServiceError::NotADirectory)
            }
            Err(error) if error.kind() == std::io::ErrorKind::IsADirectory => {
                Err(ServiceError::IsADirectory)
            }
            Err(_) => Err(ServiceError::Io),
        }
    }
}
