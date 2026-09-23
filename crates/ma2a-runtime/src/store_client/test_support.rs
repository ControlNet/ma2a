use tokio::sync::oneshot;

use super::{StoreClient, StoreCommand, channel_error};
use crate::{error::RuntimeError, store::RelayPublicationTestCommand};

impl StoreClient {
    pub(crate) async fn fail_relay_publication_after(
        &self,
        committed_spaces: usize,
    ) -> Result<(), RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.send(StoreCommand::RelayPublicationTest(
            RelayPublicationTestCommand::FailAfter(committed_spaces, reply),
        ))
        .await?;
        response.await.map_err(channel_error)
    }

    pub(crate) async fn relay_publication_pending(&self) -> Result<bool, RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.send(StoreCommand::RelayPublicationTest(
            RelayPublicationTestCommand::Pending(reply),
        ))
        .await?;
        response.await.map_err(channel_error)
    }
}
