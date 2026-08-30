//! Typed Echo v1 request, response, status, and failure contracts.

use std::fmt;

use crate::{
    EndpointId, MAX_PAYLOAD_LEN, ProtocolError, RequestEnvelope, RequestId, ResponseEnvelope,
    decode_request, decode_response, encode_request, encode_response,
};

/// Maximum Echo payload length in bytes.
pub const MAX_ECHO_PAYLOAD_LEN: usize = MAX_PAYLOAD_LEN;
/// Maximum representable end-to-end Echo duration in milliseconds.
pub const MAX_ECHO_DURATION_MS: u16 = 10_000;

/// Successful Echo response status.
#[expect(
    clippy::exhaustive_enums,
    reason = "Echo v1 has one exact success status"
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EchoStatus {
    /// The response carries the exact accepted payload.
    Ok,
}

/// Bounded result classification suitable for payload-free audit records.
#[expect(
    clippy::exhaustive_enums,
    reason = "audit classifications are a closed persisted vocabulary"
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EchoResultClass {
    /// The request completed successfully.
    Succeeded,
    /// The authenticated peer was not authorized.
    Unauthorized,
    /// The request was malformed or outside a declared bound.
    InvalidInput,
    /// The request exceeded its deadline.
    TimedOut,
    /// A concurrency bound rejected the request.
    ConcurrencyExceeded,
    /// Shutdown cancelled the request.
    Cancelled,
    /// Transport or service state was unavailable.
    Unavailable,
}

/// Closed Echo v1 failure set.
#[expect(
    clippy::exhaustive_enums,
    reason = "wire error codes require an exact closed variant set"
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EchoError {
    /// The peer uses an unsupported wire version.
    VersionMismatch,
    /// Input was malformed, noncanonical, or outside a bound.
    InvalidInput,
    /// The authenticated peer is not authorized.
    Unauthorized,
    /// The ten-second request deadline expired.
    TimedOut,
    /// A per-peer or global concurrency bound was reached.
    ConcurrencyExceeded,
    /// Runtime shutdown cancelled the operation.
    Cancelled,
    /// The transport or service is unavailable.
    Unavailable,
    /// An internal invariant failed.
    Internal,
}

impl EchoError {
    /// Returns the stable one-byte transport code.
    pub const fn code(self) -> u8 {
        match self {
            Self::VersionMismatch => 1,
            Self::InvalidInput => 2,
            Self::Unauthorized => 3,
            Self::TimedOut => 4,
            Self::ConcurrencyExceeded => 5,
            Self::Cancelled => 6,
            Self::Unavailable => 7,
            Self::Internal => 8,
        }
    }

    /// Decodes a stable one-byte transport code.
    pub const fn from_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(Self::VersionMismatch),
            2 => Some(Self::InvalidInput),
            3 => Some(Self::Unauthorized),
            4 => Some(Self::TimedOut),
            5 => Some(Self::ConcurrencyExceeded),
            6 => Some(Self::Cancelled),
            7 => Some(Self::Unavailable),
            8 => Some(Self::Internal),
            _ => None,
        }
    }

    /// Returns the payload-free audit classification.
    pub const fn result_class(self) -> EchoResultClass {
        match self {
            Self::VersionMismatch | Self::InvalidInput | Self::Internal => {
                EchoResultClass::InvalidInput
            }
            Self::Unauthorized => EchoResultClass::Unauthorized,
            Self::TimedOut => EchoResultClass::TimedOut,
            Self::ConcurrencyExceeded => EchoResultClass::ConcurrencyExceeded,
            Self::Cancelled => EchoResultClass::Cancelled,
            Self::Unavailable => EchoResultClass::Unavailable,
        }
    }
}

impl fmt::Display for EchoError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::VersionMismatch => "Echo protocol version mismatch",
            Self::InvalidInput => "invalid Echo input",
            Self::Unauthorized => "Echo request is unauthorized",
            Self::TimedOut => "Echo request timed out",
            Self::ConcurrencyExceeded => "Echo concurrency limit exceeded",
            Self::Cancelled => "Echo request was cancelled",
            Self::Unavailable => "Echo service is unavailable",
            Self::Internal => "internal Echo failure",
        })
    }
}

impl std::error::Error for EchoError {}

impl From<ProtocolError> for EchoError {
    fn from(error: ProtocolError) -> Self {
        if error == ProtocolError::VERSION_MISMATCH {
            Self::VersionMismatch
        } else if error == ProtocolError::UNAUTHORIZED {
            Self::Unauthorized
        } else if error == ProtocolError::UNAVAILABLE {
            Self::Unavailable
        } else if error == ProtocolError::INTERNAL {
            Self::Internal
        } else {
            Self::InvalidInput
        }
    }
}

/// Typed borrowed Echo request over the canonical v1 request envelope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EchoRequest<'a>(RequestEnvelope<'a>);

impl<'a> EchoRequest<'a> {
    /// Creates one bounded request targeting an Endpoint.
    ///
    /// # Errors
    /// Returns [`EchoError::InvalidInput`] when the payload exceeds the Echo bound.
    pub fn new(
        request_id: RequestId,
        target_endpoint_id: EndpointId,
        payload: &'a [u8],
    ) -> Result<Self, EchoError> {
        RequestEnvelope::echo(request_id, target_endpoint_id, payload)
            .map(Self)
            .map_err(EchoError::from)
    }

    /// Decodes exact canonical request bytes while borrowing the payload.
    ///
    /// # Errors
    /// Returns a typed Echo error when the request is malformed or unsupported.
    pub fn decode(bytes: &'a [u8]) -> Result<Self, EchoError> {
        decode_request(bytes).map(Self).map_err(EchoError::from)
    }

    /// Encodes exact canonical request bytes.
    ///
    /// # Errors
    /// Returns a typed Echo error when the request violates the canonical envelope contract.
    pub fn encode(&self) -> Result<Vec<u8>, EchoError> {
        encode_request(&self.0).map_err(EchoError::from)
    }

    /// Returns the correlation identifier.
    pub const fn request_id(&self) -> RequestId {
        self.0.request_id()
    }

    /// Returns the target Endpoint identifier.
    pub const fn target_endpoint_id(&self) -> EndpointId {
        self.0.target()
    }

    /// Returns the exact payload bytes.
    pub fn payload(&self) -> &[u8] {
        self.0.operation().payload()
    }

    /// Converts borrowed payload storage into an owned request.
    pub fn into_owned(self) -> EchoRequest<'static> {
        EchoRequest(self.0.into_owned())
    }
}

/// Typed Echo response plus authenticated responder and bounded timing metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EchoResponse<'a> {
    envelope: ResponseEnvelope<'a>,
    responder_endpoint_id: EndpointId,
    duration_ms: u16,
}

impl<'a> EchoResponse<'a> {
    /// Creates one successful response with bounded timing metadata.
    ///
    /// # Errors
    /// Returns [`EchoError::InvalidInput`] when payload or duration bounds are exceeded.
    #[expect(
        clippy::too_many_arguments,
        reason = "the response contract carries four independent wire fields"
    )]
    pub fn new(
        request_id: RequestId,
        responder_endpoint_id: EndpointId,
        payload: &'a [u8],
        duration_ms: u16,
    ) -> Result<Self, EchoError> {
        if duration_ms > MAX_ECHO_DURATION_MS {
            return Err(EchoError::InvalidInput);
        }
        Ok(Self {
            envelope: ResponseEnvelope::echo(request_id, payload).map_err(EchoError::from)?,
            responder_endpoint_id,
            duration_ms,
        })
    }

    /// Decodes canonical response bytes with authenticated transport metadata.
    ///
    /// # Errors
    /// Returns a typed Echo error for malformed, failed, or out-of-bound responses.
    pub fn decode(
        bytes: &'a [u8],
        responder_endpoint_id: EndpointId,
        duration_ms: u16,
    ) -> Result<Self, EchoError> {
        if duration_ms > MAX_ECHO_DURATION_MS {
            return Err(EchoError::InvalidInput);
        }
        let envelope = decode_response(bytes).map_err(EchoError::from)?;
        if let Some(error) = envelope.result().error() {
            return Err(EchoError::from(error));
        }
        Ok(Self {
            envelope,
            responder_endpoint_id,
            duration_ms,
        })
    }

    /// Encodes the canonical response body.
    ///
    /// # Errors
    /// Returns a typed Echo error when the response violates the canonical envelope contract.
    pub fn encode(&self) -> Result<Vec<u8>, EchoError> {
        encode_response(&self.envelope).map_err(EchoError::from)
    }

    /// Returns the correlation identifier.
    pub const fn request_id(&self) -> RequestId {
        self.envelope.request_id()
    }

    /// Returns the authenticated responder Endpoint identifier.
    pub const fn responder_endpoint_id(&self) -> EndpointId {
        self.responder_endpoint_id
    }

    /// Returns the exact echoed payload.
    pub fn payload(&self) -> &[u8] {
        self.envelope.result().echo_payload().unwrap_or_default()
    }

    /// Returns bounded processing duration in milliseconds.
    pub const fn duration_ms(&self) -> u16 {
        self.duration_ms
    }

    /// Returns successful response status.
    pub const fn status(&self) -> EchoStatus {
        EchoStatus::Ok
    }

    /// Converts borrowed payload storage into an owned response.
    pub fn into_owned(self) -> EchoResponse<'static> {
        EchoResponse {
            envelope: self.envelope.into_owned(),
            responder_endpoint_id: self.responder_endpoint_id,
            duration_ms: self.duration_ms,
        }
    }
}
