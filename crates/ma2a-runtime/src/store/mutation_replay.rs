use ma2a_core::RequestId;
use ma2a_store::{MutationReplayRecord, MutationReplayRequest, MutationReplayState, Repository};
use tokio::sync::oneshot;

use crate::error::RuntimeError;

pub(crate) enum MutationReplayCommand {
    Lookup {
        request_id: RequestId,
        reply: oneshot::Sender<Result<Option<MutationReplayState>, RuntimeError>>,
    },
    Reserve {
        request: MutationReplayRequest,
        reply: oneshot::Sender<Result<(), RuntimeError>>,
    },
    Abort {
        request_id: RequestId,
        reply: oneshot::Sender<Result<(), RuntimeError>>,
    },
    Defer {
        request_id: RequestId,
        reply: oneshot::Sender<Result<(), RuntimeError>>,
    },
    Record {
        record: MutationReplayRecord,
        reply: oneshot::Sender<Result<(), RuntimeError>>,
    },
}

pub(super) fn dispatch(repository: &mut Repository, command: MutationReplayCommand) {
    match command {
        MutationReplayCommand::Lookup { request_id, reply } => {
            drop(reply.send(repository.mutation_replay(request_id).map_err(Into::into)));
        }
        MutationReplayCommand::Reserve { request, reply } => {
            drop(
                reply.send(
                    repository
                        .reserve_mutation_replay(request.request_id(), request.fingerprint())
                        .map_err(Into::into),
                ),
            );
        }
        MutationReplayCommand::Abort { request_id, reply } => {
            drop(
                reply.send(
                    repository
                        .abort_mutation_replay(request_id)
                        .map_err(Into::into),
                ),
            );
        }
        MutationReplayCommand::Defer { request_id, reply } => {
            drop(
                reply.send(
                    repository
                        .defer_mutation_replay(request_id)
                        .map_err(Into::into),
                ),
            );
        }
        MutationReplayCommand::Record { record, reply } => {
            drop(
                reply.send(
                    repository
                        .record_mutation_replay(&record)
                        .map_err(Into::into),
                ),
            );
        }
    }
}
