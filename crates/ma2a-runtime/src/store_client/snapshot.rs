use tokio::sync::oneshot;

use crate::{
    error::RuntimeError,
    store::{StoreClient, StoreCommand, channel_error},
};

impl StoreClient {
    pub(crate) async fn snapshot(
        &self,
        endpoint_id: ma2a_core::EndpointId,
        now_ms: i64,
    ) -> Result<ma2a_store::SnapshotState, RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.send(StoreCommand::Snapshot {
            endpoint_id,
            now_ms,
            reply,
        })
        .await?;
        response.await.map_err(channel_error)?
    }
}
