use tokio::sync::oneshot;

use crate::{
    error::RuntimeError,
    store::{StoreClient, StoreCommand, channel_error},
};

impl StoreClient {
    pub(crate) async fn relay_configuration(
        &self,
    ) -> Result<ma2a_store::RelayConfiguration, RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.send(StoreCommand::RelayConfiguration { reply })
            .await?;
        response.await.map_err(channel_error)?
    }

    pub(crate) async fn set_relay_configuration(
        &self,
        configuration: ma2a_store::RelayConfiguration,
    ) -> Result<u64, RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.send(StoreCommand::SetRelayConfiguration {
            configuration,
            reply,
        })
        .await?;
        response.await.map_err(channel_error)?
    }

    pub(crate) async fn relay_authorizations(
        &self,
        local_endpoint_id: ma2a_core::EndpointId,
    ) -> Result<Vec<ma2a_core::SpaceAuthorizationView>, RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.send(StoreCommand::RelayAuthorizations {
            local_endpoint_id,
            reply,
        })
        .await?;
        response.await.map_err(channel_error)?
    }

    pub(crate) async fn reconcile_relay_activity(
        &self,
        local_endpoint_id: ma2a_core::EndpointId,
        active_spaces: Vec<ma2a_core::SpaceId>,
    ) -> Result<(u64, bool), RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.send(StoreCommand::ReconcileRelayActivity {
            local_endpoint_id,
            active_spaces,
            reply,
        })
        .await?;
        response.await.map_err(channel_error)?
    }
}
