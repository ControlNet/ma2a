use ma2a_core::RequestId;
use ma2a_store::{MutationReplayRecord, MutationReplayState};
use tokio::sync::oneshot;

use super::channel_error;
use crate::{
    error::RuntimeError,
    store::{StoreClient, StoreCommand},
};

impl StoreClient {
    pub(crate) async fn mutation_replay(
        &self,
        request_id: RequestId,
    ) -> Result<Option<MutationReplayState>, RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.send(StoreCommand::MutationReplay { request_id, reply })
            .await?;
        response.await.map_err(channel_error)?
    }

    pub(crate) async fn reserve_mutation_replay(
        &self,
        request_id: RequestId,
        fingerprint: [u8; 32],
    ) -> Result<(), RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.send(StoreCommand::ReserveMutationReplay {
            request_id,
            fingerprint,
            reply,
        })
        .await?;
        response.await.map_err(channel_error)?
    }

    pub(crate) async fn abort_mutation_replay(
        &self,
        request_id: RequestId,
    ) -> Result<(), RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.send(StoreCommand::AbortMutationReplay { request_id, reply })
            .await?;
        response.await.map_err(channel_error)?
    }

    pub(crate) async fn record_mutation_replay(
        &self,
        record: MutationReplayRecord,
    ) -> Result<(), RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.send(StoreCommand::RecordMutationReplay { record, reply })
            .await?;
        response.await.map_err(channel_error)?
    }
}
