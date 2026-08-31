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
    ) -> Result<(u64, std::collections::BTreeSet<ma2a_core::SpaceId>), RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.send(StoreCommand::RevokeOwnedSpaceMember { request, reply })
            .await?;
        response.await.map_err(channel_error)?
    }
}
