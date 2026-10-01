//! Service grants, local exposure, and streaming operation status.

use ma2a_core::{EndpointId, SpaceId};
use ma2a_store::{Committed, ExposureRecord, LocalExposure, StoreError};
use tokio::sync::oneshot;

use super::{Command, RuntimeHandle};
use crate::error::RuntimeError;

impl RuntimeHandle {
    /// Returns the registry of active and recently completed streaming operations.
    pub(crate) const fn operations(&self) -> &crate::stream_host::OperationsRegistry {
        &self.operations
    }

    /// Installs the manager of hosted Processes; only the first installation counts.
    pub(crate) fn install_processes(&self, manager: crate::processes::ProcessManager) {
        let _installed = self.processes.set(manager);
    }

    /// Returns the manager of hosted Processes once the Runtime started it.
    pub(crate) fn processes(&self) -> Option<&crate::processes::ProcessManager> {
        self.processes.get()
    }

    /// Installs the manager of hosted Agents; only the first installation counts.
    pub(crate) fn install_agents(&self, manager: crate::agents::AgentManager) {
        let _installed = self.agents.set(manager);
    }

    /// Returns the manager of hosted Agents once the Runtime started it.
    pub(crate) fn agents(&self) -> Option<&crate::agents::AgentManager> {
        self.agents.get()
    }

    /// Borrows the outbound client that opens one stream per remote service operation.
    ///
    /// The client is lent per operation rather than kept here: it holds the Endpoint, and a
    /// handle may outlive its Runtime.
    ///
    /// # Errors
    /// Returns an error when the Runtime actor has stopped.
    pub(crate) async fn service_client(&self) -> Result<ma2a_net::ServiceClient, RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::ServiceClient(reply))
            .await
            .map_err(crate::store_client::channel_error)?;
        response.await.map_err(crate::store_client::channel_error)
    }

    /// Replaces one member's service grants in an owned Space.
    ///
    /// # Errors
    /// Returns an error when the actor or store is unavailable or the commit fails.
    pub(crate) async fn grant_owned_space_member(
        &self,
        member: (SpaceId, EndpointId),
        services: [bool; 8],
    ) -> crate::store::GrantResult {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::GrantOwnedSpaceMember {
                member,
                services,
                reply,
            })
            .await
            .map_err(crate::store_client::channel_error)?;
        response.await.map_err(crate::store_client::channel_error)?
    }

    /// Adds or updates one local exposure; exposure is target consent, not membership.
    pub(crate) async fn expose_service(
        &self,
        space_id: SpaceId,
        exposure: LocalExposure,
    ) -> Result<Result<Committed<()>, StoreError>, RuntimeError> {
        self.store.expose_service(space_id, exposure).await
    }

    /// Removes one local exposure.
    pub(crate) async fn unexpose_service(
        &self,
        space_id: SpaceId,
        exposure: LocalExposure,
    ) -> Result<Result<Committed<bool>, StoreError>, RuntimeError> {
        self.store.unexpose_service(space_id, exposure).await
    }

    /// Lists every local exposure.
    pub(crate) async fn service_exposures(
        &self,
    ) -> Result<Committed<Vec<ExposureRecord>>, RuntimeError> {
        self.store.service_exposures().await
    }
}
