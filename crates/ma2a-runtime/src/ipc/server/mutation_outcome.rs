//! Replay owns execution admission, independently of response delivery.

use crate::api::{self, ApiError};

/// An execution error is not evidence that its side effects rolled back.
pub(super) enum MutationFailure {
    /// Validation failed before handing work to any effectful component.
    NotStarted(ApiError),
    /// A durable commit is known to have happened; completion failed afterwards.
    Committed { error: ApiError, revision: u64 },
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
            api::MutationEffect::Indeterminate => Self::Indeterminate(error),
        }
    }
}

pub(super) async fn record_failure(
    context: &super::ConnectionContext,
    admitted: (ma2a_store::MutationReplayRequest, u64),
    error: ApiError,
) -> Result<Vec<u8>, super::IpcError> {
    let failure = MutationFailure::from(error);
    let encoded = failure.response()?;
    match failure {
        MutationFailure::NotStarted(_) => {
            context
                .handle
                .abort_mutation_replay(admitted.0.request_id())
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
            // remains: neither a crash nor transport failure reopens execution.
            context.handle.record_mutation_replay(record).await?;
        }
    }
    Ok(encoded)
}
