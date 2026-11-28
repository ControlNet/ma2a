use ma2a_core::RequestId;
use ma2a_store::{MutationReplayRecord, MutationReplayRequest, MutationReplayState};
use tokio::sync::oneshot;

use super::channel_error;
use crate::{
    error::RuntimeError,
    store::{MutationReplayCommand, StoreClient, StoreCommand},
};

impl StoreClient {
    pub(crate) async fn mutation_replay(
        &self,
        request_id: RequestId,
    ) -> Result<Option<MutationReplayState>, RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.send(StoreCommand::MutationReplay(
            MutationReplayCommand::Lookup { request_id, reply },
        ))
        .await?;
        response.await.map_err(channel_error)?
    }

    pub(crate) async fn reserve_mutation_replay(
        &self,
        request_id: RequestId,
        fingerprint: [u8; 32],
    ) -> Result<(), RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.send(StoreCommand::MutationReplay(
            MutationReplayCommand::Reserve {
                request: MutationReplayRequest::new(request_id, fingerprint),
                reply,
            },
        ))
        .await?;
        response.await.map_err(channel_error)?
    }

    pub(crate) async fn abort_mutation_replay(
        &self,
        request_id: RequestId,
    ) -> Result<(), RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.send(StoreCommand::MutationReplay(MutationReplayCommand::Abort {
            request_id,
            reply,
        }))
        .await?;
        response.await.map_err(channel_error)?
    }

    pub(crate) async fn defer_mutation_replay(
        &self,
        request_id: RequestId,
    ) -> Result<(), RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.send(StoreCommand::MutationReplay(MutationReplayCommand::Defer {
            request_id,
            reply,
        }))
        .await?;
        response.await.map_err(channel_error)?
    }

    pub(crate) async fn record_mutation_replay(
        &self,
        record: MutationReplayRecord,
    ) -> Result<(), RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.send(StoreCommand::MutationReplay(
            MutationReplayCommand::Record { record, reply },
        ))
        .await?;
        response.await.map_err(channel_error)?
    }
}
