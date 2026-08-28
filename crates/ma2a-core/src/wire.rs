//! Bounded borrowed and owned-safe Phase 1 wire envelope types.

use std::borrow::Cow;

use crate::{EndpointId, ProtocolError, RequestId, ServiceKind};

/// Maximum Echo payload length in bytes.
pub const MAX_PAYLOAD_LEN: usize = 4_096;
/// Maximum canonical request envelope length in bytes.
pub const MAX_WIRE_LEN: usize = 4_160;

/// The exact Phase 1 protocol version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProtocolVersion {
    major: u8,
    minor: u8,
}

impl ProtocolVersion {
    /// The only accepted Phase 1 version.
    pub const V1: Self = Self { major: 1, minor: 0 };

    /// Returns the breaking wire version.
    pub const fn major(self) -> u8 {
        self.major
    }

    /// Returns the backward-compatible wire revision.
    pub const fn minor(self) -> u8 {
        self.minor
    }

    pub(crate) const fn from_parts(major: u8, minor: u8) -> Self {
        Self { major, minor }
    }
}

/// The only Phase 1 request operation, with a bounded payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestOperation<'a> {
    payload: Cow<'a, [u8]>,
}

impl<'a> RequestOperation<'a> {
    pub(crate) fn echo(payload: &'a [u8]) -> Result<Self, ProtocolError> {
        validate_payload(payload)?;
        Ok(Self {
            payload: Cow::Borrowed(payload),
        })
    }

    /// Returns `ServiceKind::ECHO`, the only Phase 1 operation discriminant.
    pub const fn service_kind(&self) -> ServiceKind {
        ServiceKind::ECHO
    }

    /// Returns the bounded Echo payload.
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    /// Converts borrowed payload storage into an owned operation value.
    pub fn into_owned(self) -> RequestOperation<'static> {
        RequestOperation {
            payload: Cow::Owned(self.payload.into_owned()),
        }
    }
}

/// A request targeting an Endpoint rather than a Space.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestEnvelope<'a> {
    version: ProtocolVersion,
    request_id: RequestId,
    target: EndpointId,
    operation: RequestOperation<'a>,
}

impl<'a> RequestEnvelope<'a> {
    /// Creates a current-version bounded Echo request.
    ///
    /// # Errors
    /// Returns `ProtocolError::INVALID_INPUT` when `payload` exceeds `MAX_PAYLOAD_LEN`.
    pub fn echo(
        request_id: RequestId,
        target: EndpointId,
        payload: &'a [u8],
    ) -> Result<Self, ProtocolError> {
        Ok(Self {
            version: ProtocolVersion::V1,
            request_id,
            target,
            operation: RequestOperation::echo(payload)?,
        })
    }

    pub(crate) const fn from_decoded(
        request_id: RequestId,
        target: EndpointId,
        operation: RequestOperation<'a>,
    ) -> Self {
        Self {
            version: ProtocolVersion::V1,
            request_id,
            target,
            operation,
        }
    }

    /// Returns the exact envelope version.
    pub const fn version(&self) -> ProtocolVersion {
        self.version
    }

    /// Returns the request correlation identifier.
    pub const fn request_id(&self) -> RequestId {
        self.request_id
    }

    /// Returns the target Endpoint identifier.
    pub const fn target(&self) -> EndpointId {
        self.target
    }

    /// Returns the closed Echo operation.
    pub const fn operation(&self) -> &RequestOperation<'a> {
        &self.operation
    }

    /// Converts borrowed payload storage into an owned envelope value.
    pub fn into_owned(self) -> RequestEnvelope<'static> {
        RequestEnvelope {
            version: self.version,
            request_id: self.request_id,
            target: self.target,
            operation: self.operation.into_owned(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ResponseKind<'a> {
    Echo(Cow<'a, [u8]>),
    Error(ProtocolError),
}

/// A closed response result discriminant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponseResult<'a> {
    kind: ResponseKind<'a>,
}

impl<'a> ResponseResult<'a> {
    pub(crate) const fn from_kind(kind: ResponseKind<'a>) -> Self {
        Self { kind }
    }

    pub(crate) const fn kind(&self) -> &ResponseKind<'a> {
        &self.kind
    }

    /// Returns the Echo payload when this is a successful result.
    pub fn echo_payload(&self) -> Option<&[u8]> {
        match &self.kind {
            ResponseKind::Echo(payload) => Some(payload),
            ResponseKind::Error(_) => None,
        }
    }

    /// Returns the typed error when this is a failed result.
    pub const fn error(&self) -> Option<ProtocolError> {
        match self.kind {
            ResponseKind::Echo(_) => None,
            ResponseKind::Error(error) => Some(error),
        }
    }
}

/// A response correlated to one request without Space metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponseEnvelope<'a> {
    version: ProtocolVersion,
    request_id: RequestId,
    result: ResponseResult<'a>,
}

impl<'a> ResponseEnvelope<'a> {
    /// Creates a current-version bounded Echo response.
    ///
    /// # Errors
    /// Returns `ProtocolError::INVALID_INPUT` when `payload` exceeds `MAX_PAYLOAD_LEN`.
    pub fn echo(request_id: RequestId, payload: &'a [u8]) -> Result<Self, ProtocolError> {
        validate_payload(payload)?;
        Ok(Self {
            version: ProtocolVersion::V1,
            request_id,
            result: ResponseResult::from_kind(ResponseKind::Echo(Cow::Borrowed(payload))),
        })
    }

    /// Creates a current-version typed error response.
    pub const fn error(request_id: RequestId, error: ProtocolError) -> Self {
        Self {
            version: ProtocolVersion::V1,
            request_id,
            result: ResponseResult::from_kind(ResponseKind::Error(error)),
        }
    }

    pub(crate) const fn from_decoded(request_id: RequestId, result: ResponseResult<'a>) -> Self {
        Self {
            version: ProtocolVersion::V1,
            request_id,
            result,
        }
    }

    /// Returns the exact envelope version.
    pub const fn version(&self) -> ProtocolVersion {
        self.version
    }

    /// Returns the request correlation identifier.
    pub const fn request_id(&self) -> RequestId {
        self.request_id
    }

    /// Returns the closed result discriminant.
    pub const fn result(&self) -> &ResponseResult<'a> {
        &self.result
    }

    /// Converts borrowed payload storage into an owned envelope value.
    pub fn into_owned(self) -> ResponseEnvelope<'static> {
        let kind = match self.result.kind {
            ResponseKind::Echo(payload) => ResponseKind::Echo(Cow::Owned(payload.into_owned())),
            ResponseKind::Error(error) => ResponseKind::Error(error),
        };
        ResponseEnvelope {
            version: self.version,
            request_id: self.request_id,
            result: ResponseResult::from_kind(kind),
        }
    }
}

const fn validate_payload(payload: &[u8]) -> Result<(), ProtocolError> {
    if payload.len() > MAX_PAYLOAD_LEN {
        Err(ProtocolError::INVALID_INPUT)
    } else {
        Ok(())
    }
}
