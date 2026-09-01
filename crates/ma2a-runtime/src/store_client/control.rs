use ma2a_core::EndpointId;
use ma2a_net::ControlRejection;
use tokio::sync::oneshot;

use crate::{
    control_sync::{
        ControlApplyOutcome, ControlAuthorizationInput, ControlExchangeInput, ControlRespondOutcome,
    },
    error::RuntimeError,
    store::{StoreClient, StoreCommand},
    store_client::channel_error,
};

impl StoreClient {
    pub(crate) async fn authorize_control(
        &self,
        local_endpoint_id: EndpointId,
        peer_endpoint_id: EndpointId,
    ) -> Result<(), ControlRejection> {
        let (reply, response) = oneshot::channel();
        self.sender
            .send(StoreCommand::AuthorizeControl {
                input: ControlAuthorizationInput {
                    local_endpoint_id,
                    remote_endpoint_id: peer_endpoint_id,
                },
                reply,
            })
            .await
            .map_err(|_| ControlRejection::Unavailable)?;
        response.await.map_err(|_| ControlRejection::Unavailable)?
    }

    pub(crate) async fn respond_control(
        &self,
        input: ControlExchangeInput,
    ) -> Result<ControlRespondOutcome, ControlRejection> {
        let (reply, response) = oneshot::channel();
        self.sender
            .send(StoreCommand::RespondControl { input, reply })
            .await
            .map_err(|_| ControlRejection::Unavailable)?;
        response.await.map_err(|_| ControlRejection::Unavailable)?
    }

    pub(crate) async fn apply_control_response(
        &self,
        input: ControlExchangeInput,
    ) -> Result<ControlApplyOutcome, RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.sender
            .send(StoreCommand::ApplyControlResponse { input, reply })
            .await
            .map_err(channel_error)?;
        response.await.map_err(channel_error)?
    }
}
