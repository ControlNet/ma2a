use ma2a_core::RequestId;

use crate::StoreError;

/// Maximum terminal local mutation decisions retained across restarts.
pub const LOCAL_MUTATION_REPLAY_MAX_ENTRIES: usize = 1_024;
/// Maximum aggregate encoded response bytes retained across restarts.
pub const LOCAL_MUTATION_REPLAY_MAX_BYTES: usize = 8 * 1_024 * 1_024;
/// Maximum encoded response bytes retained for one terminal mutation.
pub const LOCAL_MUTATION_REPLAY_MAX_RESULT_BYTES: usize = 65_536;

/// One bounded terminal local mutation decision stored without command payloads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MutationReplayRecord {
    pub(super) request_id: RequestId,
    pub(super) fingerprint: [u8; 32],
    pub(super) revision: u64,
    pub(super) response: Vec<u8>,
}

/// Correlation identity and canonical fingerprint for one local mutation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MutationReplayRequest {
    request_id: RequestId,
    fingerprint: [u8; 32],
}

/// Durable classification for one local mutation request identifier.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum MutationReplayState {
    /// The request was durably admitted but no retained terminal response is available.
    Pending([u8; 32]),
    /// The request's remote outcome is uncertain; it remains fenced to this fingerprint,
    /// conflicts on any different payload, and may be retried with the exact same command.
    Retryable([u8; 32]),
    /// The request completed and its exact terminal response is retained.
    Completed(MutationReplayRecord),
}

impl MutationReplayRequest {
    /// Creates one replay request identity from validated typed command data.
    pub const fn new(request_id: RequestId, fingerprint: [u8; 32]) -> Self {
        Self {
            request_id,
            fingerprint,
        }
    }

    /// Returns the mutation correlation identifier.
    pub const fn request_id(self) -> RequestId {
        self.request_id
    }

    /// Returns the canonical typed-command fingerprint.
    pub const fn fingerprint(self) -> [u8; 32] {
        self.fingerprint
    }
}

impl MutationReplayRecord {
    /// Creates one validated replay record from an already bounded terminal response.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the response is empty or exceeds the per-entry bound.
    pub fn new(
        request: MutationReplayRequest,
        revision: u64,
        response: Vec<u8>,
    ) -> Result<Self, StoreError> {
        if response.is_empty() || response.len() > LOCAL_MUTATION_REPLAY_MAX_RESULT_BYTES {
            return Err(StoreError::SchemaMismatch {
                detail: "local mutation replay response is outside its bound",
            });
        }
        Ok(Self {
            request_id: request.request_id,
            fingerprint: request.fingerprint,
            revision,
            response,
        })
    }

    /// Returns the mutation correlation identifier.
    pub const fn request_id(&self) -> RequestId {
        self.request_id
    }

    /// Returns the canonical typed-command fingerprint.
    pub const fn fingerprint(&self) -> [u8; 32] {
        self.fingerprint
    }

    /// Returns the original committed response revision.
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// Returns the exact encoded terminal response.
    pub fn response(&self) -> &[u8] {
        &self.response
    }
}
