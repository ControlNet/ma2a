//! Remote service contracts: names, consent, authorization, descriptors, and wire codecs.
//!
//! Every streaming service shares one frame format and one canonical `open` header. The
//! typed bodies, results, and controls below are the complete built-in vocabulary; a hosted
//! service is registered at runtime and reached through the one `service` family.

mod access;
mod agent_event;
mod agent_id;
mod agent_message;
mod agent_reply;
mod agent_request;
mod agent_state;
mod agent_task;
mod catalog;
mod cbor;
mod descriptor;
mod exec;
mod exposure;
mod files;
mod files_result;
mod frame;
mod hosted_names;
mod hosted_outcome;
mod hosted_request;
mod names;
mod open;
mod process;
mod process_id;
mod process_record;
mod process_reply;
mod process_spec;
mod process_start;
mod process_state;
mod proxy;
mod service_version;
mod status;
mod transfer;

pub use access::{ServiceAccess, ServiceAuthorizationRequest, authorize_service};
pub use agent_event::{
    AgentCard, AgentEvent, AgentEventBody, MAX_AGENT_DESCRIPTION_LEN, MAX_AGENT_TASKS,
};
pub use agent_id::{AgentId, AgentTaskId, MAX_AGENT_ID_LEN};
pub use agent_message::{
    AgentMessage, MAX_MESSAGE_BYTES, MAX_MESSAGE_PARTS, MessagePart, MessageRole,
};
pub use agent_reply::{AgentAccept, AgentReply, AgentReplyOutcome};
pub use agent_request::{AgentRequest, AgentSubmit, MAX_AGENT_PAGE, MAX_TASK_PAGE};
pub use agent_state::{AgentTaskReason, AgentTaskState};
pub use agent_task::{
    AgentTaskDetails, AgentTaskSummary, ArtifactRef, MAX_ARTIFACT_NAME_LEN, MAX_MEDIA_TYPE_LEN,
    MAX_TASK_ARTIFACTS, MAX_TASK_NOTE_LEN,
};
pub use catalog::{
    BUILTIN_VERSION, admission, builtin_description, builtin_descriptor, builtin_operations,
    stream_limits,
};
pub use descriptor::{
    MAX_MANIFEST_LEN, MAX_SERVICE_CALLS, MAX_SERVICE_DESCRIPTION_LEN, MAX_SERVICE_OPERATIONS,
    MAX_SERVICE_VERSIONS, MAX_VERSION_OPERATIONS, ServiceDescriptor, ServiceManifest,
    ServiceOrigin, select_version,
};
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
pub use hosted_names::{MAX_OPERATION_NAME_LEN, MAX_SERVICE_NAME_LEN, OperationName, ServiceName};
pub use hosted_outcome::{
    MAX_CALL_ACCEPT_LEN, MAX_CALL_RESULT_LEN, decode_call_accept, decode_call_outcome,
    encode_call_accept, encode_call_outcome,
};
pub use hosted_request::{
    DescribePage, HostedCall, MAX_CALL_BODY_LEN, MAX_DESCRIBE_PAGE, ServiceRequest, sorts_after,
};
pub use names::{
    MAX_PATH_COMPONENT_LEN, MAX_PATH_COMPONENTS, MAX_PROXY_HOST_LEN, MAX_REMOTE_PATH_LEN,
    MAX_ROOT_NAME_LEN, ProxyHost, RemotePath, RootName, valid_component,
};
pub use open::{EMPTY_ACCEPT, OpenBody, OpenHeader};
pub use process::{
    LogFrom, MAX_LOGS_READ, MAX_PROCESS_PAGE, MAX_TERMINATE_GRACE_MS, ProcessRequest, ProcessSignal,
};
pub use process_id::ProcessId;
pub use process_record::{LogWindow, ProcessDetails, ProcessSummary};
pub use process_reply::{
    ProcessAccept, ProcessReply, ProcessReplyOutcome, decode_skipped, encode_skipped,
};
pub use process_spec::{
    EndReason, MAX_ENV_NAME_LEN, MAX_PROCESS_ARGS, MAX_PROCESS_ENV, MAX_PROCESS_TEXT_LEN,
    ProcessSpec, Termination,
};
pub use process_start::{
    DEFAULT_LOG_LIMIT, MAX_LABEL_KEY_LEN, MAX_LABEL_VALUE_LEN, MAX_LOG_LIMIT, MAX_PROCESS_LABELS,
    MAX_PROCESS_NAME_LEN, MIN_LOG_LIMIT, ProcessStart,
};
pub use process_state::{ProcessReason, ProcessState};
pub use proxy::{MAX_PROXY_IDLE_MS, ProxyOutcome, ProxyRequest, ProxyResult};
pub use service_version::{Interaction, ServiceVersion};
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
