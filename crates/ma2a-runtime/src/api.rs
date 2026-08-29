//! Exact-version, framing-independent local Runtime API.

mod codec;
mod codec_fields;
mod commands;
mod events;
mod responses;
mod result_data;
mod schema;
mod snapshot;
mod snapshot_state;
mod strict_json;

use ma2a_core::{ProtocolError, RequestId};

pub(crate) use commands::UiControlCommand;
pub use commands::{COMMAND_NAMES, Command};
pub use events::{
    EVENT_NAMES, EventContinuity, RuntimeEvent, classify_event_revision, encode_event,
};
pub use responses::{ApiResponse, CommandResult, UiControlResult, encode_error, encode_response};
pub use result_data::{
    CapabilityFlags, EchoReplyView, HandshakeAuth, HandshakeState, HandshakeView,
    InteractionCapabilities, ManagementCapabilities, PrivateRelayView, PublicRelayView,
    RelayAddress, RelayCapabilities, RuntimeStatusView,
};
pub use schema::{ERROR_NAMES, LOCAL_API_SCHEMA_JSON, LOCAL_API_SCHEMA_SHA256, RESULT_NAMES};
pub use snapshot::{
    ControlSyncView, EchoSummaryView, EndpointView, ObservedRelayStateView, ReachabilityView,
    RelayCandidateView, RuntimeSnapshot, SnapshotCollections, SnapshotHeader, SpaceView,
    UiAuthView,
};
pub use snapshot_state::{ClientSnapshotState, NetworkSnapshotState, SnapshotState};

/// The only accepted Phase 1 local API version.
pub const LOCAL_API_VERSION: u16 = 1;
/// Maximum encoded request size before parsing.
pub const MAX_LOCAL_REQUEST_BYTES: usize = 16_384;
/// Maximum encoded response size.
pub const MAX_LOCAL_RESPONSE_BYTES: usize = 65_536;
/// Maximum encoded SSE event data size.
pub const MAX_LOCAL_EVENT_BYTES: usize = 16_384;
/// Maximum UTF-8 text field size unless a narrower field bound applies.
pub const MAX_TEXT_BYTES: usize = 4_096;
/// Maximum number of entities in any one response collection.
pub const MAX_COLLECTION_ITEMS: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum ReplayKind {
    #[default]
    Fresh,
    Replay,
    Conflict,
}

/// Deterministic classification for a previously observed mutation `RequestId`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ReplayDecision(ReplayKind);

impl ReplayDecision {
    /// The request identifier has not been observed.
    pub const FRESH: Self = Self(ReplayKind::Fresh);
    /// The identifier and canonical payload match a prior accepted request.
    pub const REPLAY: Self = Self(ReplayKind::Replay);
    /// The identifier was previously used with a different canonical payload.
    pub const CONFLICT: Self = Self(ReplayKind::Conflict);
}

/// Runtime callbacks invoked only after version and input validation.
pub trait RuntimeApiBoundary {
    /// Reads the authoritative revision after successful execution.
    fn state_revision(&mut self) -> u64;
    /// Classifies one mutation fingerprint without requiring persistence here.
    fn replay_decision(&mut self, request_id: RequestId, fingerprint: [u8; 32]) -> ReplayDecision;
    /// Reuses the prior result for a same-identifier, same-payload retry.
    ///
    /// # Errors
    /// Returns the exact Todo 2 protocol error produced by replay lookup.
    fn replay_result(&mut self, request_id: RequestId) -> Result<CommandResult, ProtocolError>;
    /// Executes one validated command.
    ///
    /// # Errors
    /// Returns the exact Todo 2 protocol error produced by command execution.
    fn execute(&mut self, command: &Command) -> Result<CommandResult, ProtocolError>;
}

/// A deterministic local API failure with optional version remediation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApiError {
    code: ProtocolError,
    remediation: Option<&'static str>,
}

impl ApiError {
    pub(crate) const fn invalid_input() -> Self {
        Self::new(ProtocolError::INVALID_INPUT)
    }

    pub(crate) const fn new(code: ProtocolError) -> Self {
        Self {
            code,
            remediation: None,
        }
    }

    pub(crate) const fn version_mismatch() -> Self {
        Self {
            code: ProtocolError::VERSION_MISMATCH,
            remediation: Some(
                "use a client and Runtime that both implement local API version 1; negotiation is unsupported",
            ),
        }
    }

    /// Returns the Todo 2 protocol error classification.
    pub const fn code(self) -> ProtocolError {
        self.code
    }

    /// Returns CLI-visible remediation for incompatible versions.
    pub const fn remediation(self) -> Option<&'static str> {
        self.remediation
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.code.fmt(formatter)
    }
}

impl std::error::Error for ApiError {}

/// Parses one exact-version bounded command without invoking Runtime state.
///
/// # Errors
/// Returns version mismatch or invalid input for unsupported or malformed requests.
pub fn decode_command(input: &[u8]) -> Result<Command, ApiError> {
    codec::decode_request(input)
}

/// Serializes one typed command and enforces the request bound.
///
/// # Errors
/// Returns invalid input when serialization fails or exceeds the request bound.
pub fn encode_command(command: &Command) -> Result<Vec<u8>, ApiError> {
    codec::encode_request(command)
}

/// Parses, version-gates, replay-classifies, and dispatches one local request.
///
/// # Errors
/// Returns a deterministic typed error before state access when version or input validation fails.
pub fn dispatch_request(
    input: &[u8],
    boundary: &mut impl RuntimeApiBoundary,
) -> Result<ApiResponse, ApiError> {
    let command = codec::decode_request(input)?;
    let result = match command.request_id() {
        Some(request_id) => {
            let fingerprint = codec::fingerprint(&command)?;
            match boundary.replay_decision(request_id, fingerprint).0 {
                ReplayKind::Fresh => boundary.execute(&command),
                ReplayKind::Replay => boundary.replay_result(request_id),
                ReplayKind::Conflict => return Err(ApiError::new(ProtocolError::CONFLICT)),
            }
        }
        None => boundary.execute(&command),
    }
    .map_err(ApiError::new)?;
    Ok(ApiResponse::new(
        command.request_id(),
        boundary.state_revision(),
        result,
    ))
}
