//! Closed built-in remote service contracts: names, consent, authorization, and wire codecs.
//!
//! Every streaming service shares one frame format and one canonical `open` header. The
//! typed bodies, results, and controls below are the complete Phase 2 vocabulary; nothing
//! is registered at runtime.

mod access;
mod cbor;
mod exec;
mod exposure;
mod files;
mod files_result;
mod frame;
mod names;
mod open;
mod process_spec;
mod proxy;
mod status;
mod transfer;

pub use access::{ServiceAccess, ServiceAuthorizationRequest, authorize_service};
pub use exec::{
    ExecRequest, MAX_OPERATION_LIFETIME_MS, MAX_TERM_LEN, ProcessOutcome, ProcessResult,
    ShellRequest, TerminalSize,
};
pub use exposure::{ExportedRoot, ProxyHostPattern, ProxyRule, RootAccess, SpaceExposureView};
pub use files::{
    DirEntry, FileKind, FileMetadata, FilesOutcome, FilesRequest, FilesResult, MAX_LIST_PAGE,
    MAX_SMALL_FILE_LEN, WriteMode,
};
pub use frame::{
    Channel, FRAME_HEADER_LEN, FrameHeader, FrameKind, MAX_DATA_CHUNK_LEN, MAX_HEADER_FRAME_LEN,
    MAX_OUTCOME_FRAME_LEN,
};
pub use names::{
    MAX_PATH_COMPONENT_LEN, MAX_PATH_COMPONENTS, MAX_PROXY_HOST_LEN, MAX_REMOTE_PATH_LEN,
    MAX_ROOT_NAME_LEN, ProxyHost, RemotePath, RootName,
};
pub use open::{EMPTY_ACCEPT, OpenBody, OpenHeader, admission};
pub use process_spec::{
    EndReason, MAX_ENV_NAME_LEN, MAX_PROCESS_ARGS, MAX_PROCESS_ENV, MAX_PROCESS_TEXT_LEN,
    ProcessSpec, Termination,
};
pub use proxy::{MAX_PROXY_IDLE_MS, ProxyOutcome, ProxyRequest, ProxyResult};
pub use status::ServiceError;
pub use transfer::{
    ContentIdentity, TransferAccept, TransferCommit, TransferOutcome, TransferRequest,
    TransferResource, TransferResult,
};

/// Encodes the family-independent failed `outcome` payload `{0: code, 1: null}`.
///
/// # Errors
/// Returns [`crate::ProtocolError::INVALID_INPUT`] only if the bounded encoder fails.
pub fn failed_outcome(error: ServiceError) -> Result<Vec<u8>, crate::ProtocolError> {
    cbor::encode_outcome::<()>(Err(error), |(), _| Ok(()))
}

/// Returns the status of any family's `outcome` payload: `None` when it succeeded.
///
/// # Errors
/// Returns [`crate::ProtocolError::INVALID_INPUT`] for a malformed envelope.
pub fn outcome_status(payload: &[u8]) -> Result<Option<ServiceError>, crate::ProtocolError> {
    cbor::outcome_status(payload)
}
