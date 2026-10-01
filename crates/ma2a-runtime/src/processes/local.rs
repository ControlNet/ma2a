//! Processes the local Endpoint creates for itself: Agent instances.
//!
//! They are ordinary Processes with the local Endpoint as creator, so no remote peer can
//! ever see or manage them, and withdrawal convergence never ends them.

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
        let (creation, credentials) = self.create((self.0.local, request_id), &start).await?;
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
        self.open_live((None, id), SupervisorRequest::Attach(from, true))
            .await
    }

    /// Returns one Process record, whoever created it.
    pub(crate) async fn local_record(
        &self,
        id: ProcessId,
    ) -> Result<Option<ProcessRecord>, ServiceError> {
        self.record(id).await
    }
}
