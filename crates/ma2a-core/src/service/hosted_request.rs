//! Service v1 requests: discovery of what a target serves this caller, and calls to the
//! services third-party programs host through it.

use crate::{ProtocolError, space_codec::Decoder};

use super::{
    MAX_OPERATION_NAME_LEN, MAX_SERVICE_NAME_LEN, OperationName, ServiceAccess, ServiceDescriptor,
    ServiceName, ServiceOrigin,
    cbor::{self, Writer},
};

/// Most descriptors one `describe` page returns.
pub const MAX_DESCRIBE_PAGE: usize = 16;
/// Longest opaque request body of one call, carried in the `open` header.
pub const MAX_CALL_BODY_LEN: usize = 8_192;

/// One call to a hosted service: exactly one version and one of its operations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostedCall {
    service: ServiceName,
    version: u16,
    operation: OperationName,
    body: Vec<u8>,
}

impl HostedCall {
    /// Validates one call: a non-zero version and a body of at most 8 KiB.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] outside those bounds.
    pub fn new(
        (service, version): (ServiceName, u16),
        operation: OperationName,
        body: Vec<u8>,
    ) -> Result<Self, ProtocolError> {
        if version == 0 || body.len() > MAX_CALL_BODY_LEN {
            return Err(ProtocolError::INVALID_INPUT);
        }
        Ok(Self {
            service,
            version,
            operation,
            body,
        })
    }

    /// Returns the addressed service.
    pub const fn service(&self) -> &ServiceName {
        &self.service
    }

    /// Returns the exact protocol version the caller speaks.
    pub const fn version(&self) -> u16 {
        self.version
    }

    /// Returns the operation.
    pub const fn operation(&self) -> &OperationName {
        &self.operation
    }

    /// Returns the opaque request body.
    pub fn body(&self) -> &[u8] {
        &self.body
    }
}

/// One Service v1 operation.
#[expect(
    clippy::exhaustive_enums,
    reason = "Service v1 operations are a closed wire vocabulary"
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServiceRequest {
    /// One page of the services this caller may use, after this name.
    Describe(Option<String>),
    /// One call to a hosted service.
    Call(HostedCall),
}

impl ServiceRequest {
    /// Returns the complete access this operation needs.
    pub fn access(&self) -> ServiceAccess {
        match self {
            Self::Describe(_) => ServiceAccess::Discover,
            Self::Call(call) => ServiceAccess::Hosted(call.service.clone()),
        }
    }

    /// Returns whether this operation can change target state: every call may.
    pub const fn is_mutation(&self) -> bool {
        matches!(self, Self::Call(_))
    }

    pub(crate) fn encode(&self, writer: &mut Writer) -> Result<(), ProtocolError> {
        match self {
            Self::Describe(after) => {
                writer.array(2)?.uint(0).optional_text(after.as_deref())?;
            }
            Self::Call(call) => {
                writer
                    .array(5)?
                    .uint(1)
                    .text(call.service.as_str())?
                    .uint(u64::from(call.version))
                    .text(call.operation.as_str())?
                    .bytes(&call.body)?;
            }
        }
        Ok(())
    }

    pub(crate) fn decode(decoder: &mut Decoder<'_>) -> Result<Self, ProtocolError> {
        let request = match (decoder.array(5)?, decoder.uint()?) {
            (2, 0) => Self::Describe(cbor::optional_text(decoder, MAX_SERVICE_NAME_LEN)?),
            (5, 1) => {
                let service = ServiceName::decode_from(decoder)?;
                let version = cbor::u16_value(decoder)?;
                let operation = OperationName::new(decoder.text(MAX_OPERATION_NAME_LEN)?)?;
                let body = decoder.bytes(MAX_CALL_BODY_LEN)?.to_vec();
                Self::Call(HostedCall::new((service, version), operation, body)?)
            }
            _ => return Err(ProtocolError::INVALID_INPUT),
        };
        Ok(request)
    }
}

/// The successful outcome of `describe`: descriptors and the name to continue after.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DescribePage {
    /// At most [`MAX_DESCRIBE_PAGE`] descriptors, built-ins first, then hosted by name.
    pub descriptors: Vec<ServiceDescriptor>,
    /// The name to pass as `after` for the next page, when there is one.
    pub next: Option<String>,
}

impl DescribePage {
    /// Creates one page.
    pub const fn new(descriptors: Vec<ServiceDescriptor>, next: Option<String>) -> Self {
        Self { descriptors, next }
    }

    /// Encodes one `describe` outcome envelope.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] for an oversized page.
    pub fn encode_outcome(
        outcome: Result<&Self, super::ServiceError>,
    ) -> Result<Vec<u8>, ProtocolError> {
        cbor::encode_outcome(outcome, |page, writer| {
            if page.descriptors.len() > MAX_DESCRIBE_PAGE {
                return Err(ProtocolError::INVALID_INPUT);
            }
            writer.array(2)?.array(page.descriptors.len())?;
            for descriptor in &page.descriptors {
                descriptor.encode(writer)?;
            }
            writer.optional_text(page.next.as_deref())?;
            Ok(())
        })
    }

    /// Decodes one `describe` outcome envelope.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] for malformed or noncanonical input.
    pub fn decode_outcome(
        bytes: &[u8],
    ) -> Result<Result<Self, super::ServiceError>, ProtocolError> {
        cbor::decode_outcome(bytes, |decoder| {
            decoder.array_exact(2)?;
            let count = decoder.array(MAX_DESCRIBE_PAGE)?;
            let descriptors = (0..count)
                .map(|_| ServiceDescriptor::decode(decoder))
                .collect::<Result<Vec<_>, _>>()?;
            let next = cbor::optional_text(decoder, MAX_SERVICE_NAME_LEN)?;
            Ok(Self { descriptors, next })
        })
    }
}

/// Returns whether `origin` sorts after the `after` cursor of a page.
///
/// Built-ins come first, in catalog order, then hosted services by name. Built-in names are
/// reserved, so a name alone says which of the two it is.
pub fn sorts_after(origin: &ServiceOrigin, after: Option<&str>) -> bool {
    let Some(after) = after else {
        return true;
    };
    let cursor = crate::ServiceKind::from_name(after)
        .map_or((true, 0, after), |kind| (false, kind.code(), ""));
    let own = match origin {
        ServiceOrigin::Builtin(kind) => (false, kind.code(), ""),
        ServiceOrigin::Hosted(name) => (true, 0, name.as_str()),
    };
    own > cursor
}
