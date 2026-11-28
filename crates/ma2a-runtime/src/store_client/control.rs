use crate::control_sync::ControlFailure;
use ma2a_core::EndpointId;
use tokio::sync::oneshot;

use crate::{
    control_sync::{
        ControlApplyOutcome, ControlAuthorizationInput, ControlExchangeInput, ControlRespondOutcome,
    },
    store::{StoreClient, StoreCommand},
    store_client::channel_error,
};

impl StoreClient {
    pub(crate) async fn authorize_control(
        &self,
        local_endpoint_id: EndpointId,
        peer_endpoint_id: EndpointId,
    ) -> Result<(), ControlFailure> {
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
            .map_err(channel_error)?;
        response.await.map_err(channel_error)?
    }

    pub(crate) async fn respond_control(
        &self,
        input: ControlExchangeInput,
    ) -> Result<ControlRespondOutcome, ControlFailure> {
        let (reply, response) = oneshot::channel();
        self.sender
            .send(StoreCommand::RespondControl { input, reply })
            .await
            .map_err(channel_error)?;
        response.await.map_err(channel_error)?
    }

    pub(crate) async fn admit_control_response(
        &self,
        (input, requested): (ControlExchangeInput, Vec<ma2a_core::SpaceId>),
        deadline: tokio::time::Instant,
    ) -> Result<oneshot::Receiver<Result<ControlApplyOutcome, ControlFailure>>, ControlFailure>
    {
        let permit = self.sender.reserve().await.map_err(channel_error)?;
        let (reply, response) = oneshot::channel();
        permit.send(StoreCommand::ApplyControlResponse {
            input: (input, requested),
            deadline,
            reply,
        });
        Ok(response)
    }
}
