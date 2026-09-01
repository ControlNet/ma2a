use ma2a_core::RequestId;
use ma2a_store::{MutationReplayRecord, MutationReplayRequest, MutationReplayState, Repository};
use tokio::sync::oneshot;

use crate::error::RuntimeError;

pub(super) fn lookup(
    repository: &Repository,
    request_id: RequestId,
    reply: oneshot::Sender<Result<Option<MutationReplayState>, RuntimeError>>,
) {
    drop(reply.send(repository.mutation_replay(request_id).map_err(Into::into)));
}

pub(super) fn reserve(
    repository: &mut Repository,
    request: MutationReplayRequest,
    reply: oneshot::Sender<Result<(), RuntimeError>>,
) {
    drop(
        reply.send(
            repository
                .reserve_mutation_replay(request.request_id(), request.fingerprint())
                .map_err(Into::into),
        ),
    );
}

pub(super) fn abort(
    repository: &mut Repository,
    request_id: RequestId,
    reply: oneshot::Sender<Result<(), RuntimeError>>,
) {
    drop(
        reply.send(
            repository
                .abort_mutation_replay(request_id)
                .map_err(Into::into),
        ),
    );
}

pub(super) fn complete(
    repository: &mut Repository,
    record: &MutationReplayRecord,
    reply: oneshot::Sender<Result<(), RuntimeError>>,
) {
    drop(
        reply.send(
            repository
                .record_mutation_replay(record)
                .map_err(Into::into),
        ),
    );
}
