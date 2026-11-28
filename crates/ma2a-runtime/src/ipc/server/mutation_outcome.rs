//! Replay owns execution admission, independently of response delivery.

use ma2a_core::{ProtocolError, RequestId};

use crate::api::{self, ApiError};

pub(super) enum ReplayAdmission {
    Answer(Vec<u8>),
    Proceed { retryable_fence: bool },
}

/// Answers a request ID that was already admitted, or durably reserves a new one and returns
/// `Proceed` so that it executes.
pub(super) async fn admit(
    context: &super::ConnectionContext,
    (request_id, fingerprint): (RequestId, [u8; 32]),
    initial_retryable: bool,
) -> Result<ReplayAdmission, super::IpcError> {
    let Some(entry) = context.handle.mutation_replay(request_id).await? else {
        if initial_retryable {
            context
                .handle
                .reserve_retryable_mutation_replay(request_id, fingerprint)
                .await?;
        } else {
            context
                .handle
                .reserve_mutation_replay(request_id, fingerprint)
                .await?;
        }
        return Ok(ReplayAdmission::Proceed {
            retryable_fence: false,
        });
    };
    let refusal = match entry {
        ma2a_store::MutationReplayState::Completed(entry) if entry.fingerprint() == fingerprint => {
            return Ok(ReplayAdmission::Answer(entry.response().to_vec()));
        }
        ma2a_store::MutationReplayState::Retryable(found) if found == fingerprint => {
            context
                .handle
                .reserve_mutation_replay(request_id, fingerprint)
                .await?;
            return Ok(ReplayAdmission::Proceed {
                retryable_fence: true,
            });
        }
        ma2a_store::MutationReplayState::Pending(found) if found == fingerprint => {
            ProtocolError::UNAVAILABLE
        }
        ma2a_store::MutationReplayState::Pending(_)
        | ma2a_store::MutationReplayState::Retryable(_)
        | ma2a_store::MutationReplayState::Completed(_) => ProtocolError::CONFLICT,
        _ => ProtocolError::UNAVAILABLE,
    };
    Ok(ReplayAdmission::Answer(api::encode_error(ApiError::new(
        refusal,
    ))?))
}

/// An execution error is not evidence that its side effects rolled back.
pub(super) enum MutationFailure {
    /// Validation failed before handing work to any effectful component.
    NotStarted(ApiError),
    /// A durable commit is known to have happened; completion failed afterwards.
    Committed { error: ApiError, revision: u64 },
    /// Remote execution may have committed; keep the request ID fenced for exact retry only.
    Retryable(ApiError),
    /// Execution started, but its durable/external outcome cannot be proven.
    Indeterminate(ApiError),
}

impl MutationFailure {
    pub(super) fn response(&self) -> Result<Vec<u8>, ApiError> {
        let error = match self {
            Self::NotStarted(error) => *error,
            Self::Committed { error, .. } => ApiError::with_remediation(
                error.code(),
                "mutation committed; completion failed; inspect current state; this request will not execute again",
            ),
            Self::Retryable(error) => ApiError::with_remediation(
                error.code(),
                "remote outcome may include side effects; inspect current state or retry the exact same request",
            ),
            Self::Indeterminate(error) => ApiError::with_remediation(
                error.code(),
                "mutation outcome may include side effects; inspect current state; this request will not execute again",
            ),
        };
        api::encode_error(error)
    }
}

impl From<ApiError> for MutationFailure {
    fn from(error: ApiError) -> Self {
        match error.effect {
            api::MutationEffect::NotStarted => Self::NotStarted(error),
            api::MutationEffect::Committed(revision) => Self::Committed { error, revision },
            api::MutationEffect::Retryable => Self::Retryable(error),
            api::MutationEffect::Indeterminate => Self::Indeterminate(error),
        }
    }
}

pub(super) async fn record_failure(
    context: &super::ConnectionContext,
    admitted: (ma2a_store::MutationReplayRequest, u64, bool),
    error: ApiError,
) -> Result<Vec<u8>, super::IpcError> {
    let failure = MutationFailure::from(error);
    let encoded = failure.response()?;
    match failure {
        MutationFailure::NotStarted(_) => {
            if admitted.2 {
                context
                    .handle
                    .abort_mutation_replay(admitted.0.request_id())
                    .await?;
            } else {
                context
                    .handle
                    .release_mutation_replay(admitted.0.request_id())
                    .await?;
            }
        }
        MutationFailure::Retryable(_) => {
            context
                .handle
                .defer_mutation_replay(admitted.0.request_id())
                .await?;
        }
        failure => {
            let revision = match failure {
                MutationFailure::Committed { revision, .. } => revision,
                _ => admitted.1,
            };
            let record =
                ma2a_store::MutationReplayRecord::new(admitted.0, revision, encoded.clone())
                    .map_err(crate::RuntimeError::from)?;
            // If completion persistence fails, the durable Pending reservation
            // remains: a transport failure answers unavailable while running,
            // and a restart only reopens an exact retry if the fence was already
            // retryable (such as an initial space_redeem).
            context.handle.record_mutation_replay(record).await?;
        }
    }
    Ok(encoded)
}
