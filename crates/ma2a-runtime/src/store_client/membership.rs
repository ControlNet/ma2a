use tokio::sync::oneshot;

use crate::{
    error::RuntimeError,
    store::{StoreClient, StoreCommand, channel_error},
};

impl StoreClient {
    pub(crate) async fn create_owned_space(
        &self,
        creation: ma2a_store::SpaceCreation,
    ) -> Result<ma2a_store::CreatedSpace, RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.send(StoreCommand::CreateOwnedSpace { creation, reply })
            .await?;
        response.await.map_err(channel_error)?
    }

    pub(crate) async fn revoke_owned_space_member(
        &self,
        request: crate::store::OwnedMemberRevocation,
    ) -> Result<crate::store::RemovedMember, RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.send(StoreCommand::RevokeOwnedSpaceMember { request, reply })
            .await?;
        response.await.map_err(channel_error)?
    }

    pub(crate) async fn memberships(
        &self,
        local_endpoint_id: ma2a_core::EndpointId,
    ) -> Result<std::collections::BTreeSet<ma2a_core::SpaceId>, RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.send(StoreCommand::Memberships {
            local_endpoint_id,
            reply,
        })
        .await?;
        response.await.map_err(channel_error)?
    }

    pub(crate) async fn load_space_chain(
        &self,
        space_id: ma2a_core::SpaceId,
    ) -> Result<Option<ma2a_core::SpaceChain>, RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.send(StoreCommand::LoadSpaceChain { space_id, reply })
            .await?;
        response.await.map_err(channel_error)?
    }

    pub(crate) async fn persist_departure(
        &self,
        chain: ma2a_core::SpaceChain,
        local_endpoint_id: ma2a_core::EndpointId,
    ) -> Result<(u64, std::collections::BTreeSet<ma2a_core::SpaceId>), RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.send(StoreCommand::PersistDeparture {
            chain: Box::new(chain),
            local_endpoint_id,
            reply,
        })
        .await?;
        response.await.map_err(channel_error)?
    }
}
