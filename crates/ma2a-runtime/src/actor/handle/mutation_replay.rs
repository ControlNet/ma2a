use ma2a_core::RequestId;
use ma2a_store::{MutationReplayRecord, MutationReplayState};

use super::RuntimeHandle;
use crate::error::RuntimeError;

impl RuntimeHandle {
    pub(crate) async fn mutation_replay(
        &self,
        request_id: RequestId,
    ) -> Result<Option<MutationReplayState>, RuntimeError> {
        self.store.mutation_replay(request_id).await
    }

    pub(crate) async fn reserve_mutation_replay(
        &self,
        request_id: RequestId,
        fingerprint: [u8; 32],
    ) -> Result<(), RuntimeError> {
        self.store
            .reserve_mutation_replay(request_id, fingerprint)
            .await
    }

    pub(crate) async fn abort_mutation_replay(
        &self,
        request_id: RequestId,
    ) -> Result<(), RuntimeError> {
        self.store.abort_mutation_replay(request_id).await
    }

    pub(crate) async fn record_mutation_replay(
        &self,
        record: MutationReplayRecord,
    ) -> Result<(), RuntimeError> {
        self.store.record_mutation_replay(record).await
    }
}
