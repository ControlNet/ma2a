use ma2a_core::{EndpointId, SpaceId};
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::{
    error::{RuntimeError, RuntimeErrorKind},
    state::{RuntimeEvent, RuntimeStatus},
};

use super::{Command, ShutdownAck};

mod control;
mod mutation_replay;

/// Bounded command and event handle for the single-owner Runtime actor.
#[derive(Clone, Debug)]
pub struct RuntimeHandle {
    pub(crate) commands: mpsc::Sender<Command>,
    pub(crate) store: crate::store::StoreClient,
    events: broadcast::Sender<RuntimeEvent>,
    echo_audit: crate::echo_audit::EchoAuditLog,
    echo_metrics: ma2a_net::EchoMetrics,
    #[cfg(test)]
    control_schedule_events:
        std::sync::Arc<std::sync::Mutex<Vec<crate::control_sync::ControlRoundTrigger>>>,
}

impl RuntimeHandle {
    /// Resolves once the Runtime actor has stopped accepting commands.
    ///
    /// The actor owns the only receiver, so this completes exactly when the actor
    /// task has ended. A Runtime that has stopped can still be connected to, and
    /// every command would then fail after the transport had already accepted the
    /// request; callers use this to stop serving instead of answering nothing.
    pub async fn stopped(&self) {
        self.commands.closed().await;
    }

    /// Returns complete signed details for one local Space.
    ///
    /// # Errors
    /// Returns an error when the actor or store is unavailable.
    pub async fn space_details(
        &self,
        id: SpaceId,
    ) -> Result<Option<crate::api::SpaceDetailsView>, RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::SpaceDetails(id, reply))
            .await
            .map_err(crate::store_client::channel_error)?;
        response.await.map_err(crate::store_client::channel_error)?
    }
    pub(crate) async fn snapshot_stamp(
        &self,
    ) -> Result<crate::api::SnapshotStampView, RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::SnapshotStamp(reply))
            .await
            .map_err(crate::store_client::channel_error)?;
        response.await.map_err(crate::store_client::channel_error)?
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the handle owns independently constructed command, event, and observability channels"
    )]
    pub(crate) const fn new(
        commands: mpsc::Sender<Command>,
        store: crate::store::StoreClient,
        events: broadcast::Sender<RuntimeEvent>,
        echo_audit: crate::echo_audit::EchoAuditLog,
        echo_metrics: ma2a_net::EchoMetrics,
        #[cfg(test)] control_schedule_events: std::sync::Arc<
            std::sync::Mutex<Vec<crate::control_sync::ControlRoundTrigger>>,
        >,
    ) -> Self {
        Self {
            commands,
            store,
            events,
            echo_audit,
            echo_metrics,
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

    /// Returns the authoritative public Runtime snapshot through the actor mailbox.
    ///
    /// # Errors
    /// Returns [`RuntimeError`] when persistence or the Runtime actor is unavailable.
    pub async fn snapshot(&self) -> Result<crate::api::RuntimeSnapshot, RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::Snapshot(reply))
            .await
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Channel))?;
        response
            .await
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Channel))?
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

    pub(crate) async fn create_owned_space(&self, name: String) -> Result<SpaceId, RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::CreateOwnedSpace { name, reply })
            .await
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Channel))?;
        response
            .await
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Channel))?
    }

    pub(crate) async fn revoke_owned_space_member(
        &self,
        space_id: SpaceId,
        endpoint_id: EndpointId,
    ) -> Result<u64, RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::RevokeOwnedSpaceMember {
                space_id,
                endpoint_id,
                reply,
            })
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

    /// Subscribes to bounded best-effort Runtime events.
    pub fn subscribe(&self) -> broadcast::Receiver<RuntimeEvent> {
        self.events.subscribe()
    }

    /// Calls the encrypted typed Echo service without semantic retry.
    ///
    /// # Errors
    /// Returns a typed Echo failure for invalid input, authorization, timeout, or transport errors.
    #[expect(
        clippy::too_many_arguments,
        reason = "the public Echo call requires correlation, target identity, and payload"
    )]
    pub async fn echo(
        &self,
        request_id: ma2a_core::RequestId,
        target: EndpointId,
        payload: &[u8],
    ) -> Result<ma2a_core::EchoResponse<'static>, ma2a_core::EchoError> {
        if payload.len() > ma2a_core::MAX_ECHO_PAYLOAD_LEN {
            return Err(ma2a_core::EchoError::InvalidInput);
        }
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::Echo {
                request_id,
                target,
                payload: payload.to_vec(),
                reply,
            })
            .await
            .map_err(|_| ma2a_core::EchoError::Cancelled)?;
        response
            .await
            .map_err(|_| ma2a_core::EchoError::Cancelled)?
    }

    /// Returns the bounded payload-free Echo audit snapshot.
    pub fn echo_audit(&self) -> Vec<crate::EchoAuditRecord> {
        self.echo_audit.snapshot()
    }

    /// Returns body-processing metrics for authorization-ordering verification.
    pub fn echo_metrics(&self) -> ma2a_net::EchoMetricsSnapshot {
        self.echo_metrics.snapshot()
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
