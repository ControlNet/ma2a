//! Control-synchronisation entry points on the Runtime handle.

use ma2a_core::EndpointId;
use tokio::sync::oneshot;

use crate::{
    actor::{Command, RuntimeHandle},
    error::{RuntimeError, RuntimeErrorKind},
};

impl RuntimeHandle {
    /// Schedules one bounded control reconciliation round.
    ///
    /// # Errors
    /// Returns [`RuntimeError`] when the Runtime actor has stopped.
    pub async fn sync_control(&self) -> Result<u64, RuntimeError> {
        self.sync_control_scope(None).await
    }

    pub(crate) async fn sync_control_with(&self, peer: EndpointId) -> Result<u64, RuntimeError> {
        self.sync_control_scope(Some(peer)).await
    }

    async fn sync_control_scope(&self, peer: Option<EndpointId>) -> Result<u64, RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::SyncControl { peer, reply })
            .await
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Channel))?;
        response
            .await
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Channel))?
    }

    pub(crate) async fn control_sync_status(
        &self,
        peer: EndpointId,
    ) -> Result<ma2a_store::Committed<bool>, RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::ControlSyncStatus { peer, reply })
            .await
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Channel))?;
        response
            .await
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Channel))
    }

    #[cfg(test)]
    pub(crate) fn control_schedules(&self) -> Vec<crate::control_sync::ControlRoundTrigger> {
        self.control_schedule_events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}
