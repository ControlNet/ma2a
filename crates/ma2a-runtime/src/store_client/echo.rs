use ma2a_core::{EchoError, EndpointId};
use tokio::sync::oneshot;

use crate::store::{StoreClient, StoreCommand};

impl StoreClient {
    pub(crate) async fn authorize_echo(
        &self,
        local_endpoint_id: EndpointId,
        peer_endpoint_id: EndpointId,
    ) -> Result<(), EchoError> {
        let (reply, response) = oneshot::channel();
        self.sender
            .send(StoreCommand::AuthorizeEcho {
                local_endpoint_id,
                peer_endpoint_id,
                reply,
            })
            .await
            .map_err(|_| EchoError::Unavailable)?;
        response.await.map_err(|_| EchoError::Unavailable)?
    }
}
