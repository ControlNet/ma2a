use ma2a_core::{EndpointId, SpaceId};
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::{
    error::{RuntimeError, RuntimeErrorKind},
    state::{RuntimeEvent, RuntimeStatus},
};

use super::{Command, ShutdownAck};

/// Bounded command and event handle for the single-owner Runtime actor.
#[derive(Clone, Debug)]
pub struct RuntimeHandle {
    pub(crate) commands: mpsc::Sender<Command>,
    events: broadcast::Sender<RuntimeEvent>,
    #[cfg(test)]
    control_schedule_events:
        std::sync::Arc<std::sync::Mutex<Vec<crate::control_sync::ControlRoundTrigger>>>,
}

impl RuntimeHandle {
    pub(crate) const fn new(
        commands: mpsc::Sender<Command>,
        events: broadcast::Sender<RuntimeEvent>,
        #[cfg(test)] control_schedule_events: std::sync::Arc<
            std::sync::Mutex<Vec<crate::control_sync::ControlRoundTrigger>>,
        >,
    ) -> Self {
        Self {
            commands,
            events,
            #[cfg(test)]
            control_schedule_events,
        }
    }

    /// Returns an authoritative state snapshot through the actor mailbox.
    ///
    /// # Errors
    /// Returns [`RuntimeError`] when the Runtime actor has stopped.
    pub async fn status(&self) -> Result<RuntimeStatus, RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::Status(reply))
            .await
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Channel))?;
        response
            .await
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Channel))
    }

    /// Replaces the observed valid membership set without rebuilding the Endpoint.
    ///
    /// # Errors
    /// Returns [`RuntimeError`] when persistence fails or the Runtime actor has stopped.
    pub async fn observe_memberships(
        &self,
        memberships: Vec<SpaceId>,
    ) -> Result<u64, RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::ObserveMemberships { memberships, reply })
            .await
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Channel))?;
        response
            .await
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Channel))?
    }

    pub(crate) async fn adopt_revision(&self, revision: u64) -> Result<u64, RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::AdoptRevision { revision, reply })
            .await
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Channel))?;
        response
            .await
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Channel))
    }

    /// Schedules one bounded control reconciliation round.
    ///
    /// # Errors
    /// Returns [`RuntimeError`] when the Runtime actor has stopped.
    pub async fn sync_control(&self) -> Result<u64, RuntimeError> {
        self.sync_control_scope(None).await
    }

    /// Signs and commits the next manifest through the Runtime owner.
    ///
    /// # Errors
    /// Returns [`RuntimeError`] when the actor or owned Store rejects the update.
    pub async fn advance_owned_space(
        &self,
        update: ma2a_store::OwnedSpaceUpdate,
    ) -> Result<u64, RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::AdvanceOwnedSpace { update, reply })
            .await
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Channel))?;
        response
            .await
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Channel))?
    }

    /// Publishes current Iroh address observations into every local Space.
    ///
    /// # Errors
    /// Returns [`RuntimeError`] when observations, validation, persistence, or the actor fails.
    pub async fn publish_address(&self) -> Result<u64, RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::PublishAddress(reply))
            .await
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Channel))?;
        response
            .await
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Channel))?
    }

    /// Publishes private-relay advertisements for the configured served Spaces.
    ///
    /// # Errors
    /// Returns [`RuntimeError`] when signing, validation, persistence, or the actor fails.
    pub async fn publish_relay_advertisements(
        &self,
        config: ma2a_net::PrivateRelayProviderConfig,
        expires_at_ms: u64,
    ) -> Result<u64, RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::PublishRelayAdvertisements {
                config,
                expires_at_ms,
                reply,
            })
            .await
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Channel))?;
        response
            .await
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Channel))?
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

    pub(crate) async fn control_sync_status(&self, peer: EndpointId) -> Result<bool, RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::ControlSyncStatus { peer, reply })
            .await
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Channel))?;
        response
            .await
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Channel))
    }

    /// Subscribes to bounded best-effort Runtime events.
    pub fn subscribe(&self) -> broadcast::Receiver<RuntimeEvent> {
        self.events.subscribe()
    }

    #[cfg(test)]
    pub(crate) fn control_schedules(&self) -> Vec<crate::control_sync::ControlRoundTrigger> {
        self.control_schedule_events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub(crate) async fn shutdown(&self) -> Result<ShutdownAck, RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::Shutdown(reply))
            .await
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Channel))?;
        response
            .await
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Channel))
    }
}
