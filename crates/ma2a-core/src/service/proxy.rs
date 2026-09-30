//! Proxy v1 request and result contracts.

use crate::{ProtocolError, space_codec::Decoder};

use super::{
    EndReason, MAX_PROXY_HOST_LEN, ProxyHost, ServiceAccess, ServiceError,
    cbor::{self, Writer},
};

/// Maximum idle timeout of one proxy stream: one hour.
pub const MAX_PROXY_IDLE_MS: u64 = 3_600_000;

/// One TCP connection opened by the target to an exact destination.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProxyRequest {
    host: ProxyHost,
    port: u16,
    idle_timeout_ms: u64,
}

impl ProxyRequest {
    /// Validates a destination and idle timeout.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] for port zero or an invalid idle timeout.
    pub fn new(host: ProxyHost, port: u16, idle_timeout_ms: u64) -> Result<Self, ProtocolError> {
        if port == 0 || idle_timeout_ms == 0 || idle_timeout_ms > MAX_PROXY_IDLE_MS {
            return Err(ProtocolError::INVALID_INPUT);
        }
        Ok(Self {
            host,
            port,
            idle_timeout_ms,
        })
    }

    /// Returns the destination host exactly as requested.
    pub const fn host(&self) -> &ProxyHost {
        &self.host
    }

    /// Returns the destination port.
    pub const fn port(&self) -> u16 {
        self.port
    }

    /// Returns the idle timeout after which the stream is closed.
    pub const fn idle_timeout_ms(&self) -> u64 {
        self.idle_timeout_ms
    }

    /// Returns the complete access this stream needs.
    pub fn access(&self) -> ServiceAccess {
        ServiceAccess::ProxyConnect(self.host.clone(), self.port)
    }

    pub(crate) fn encode(&self, writer: &mut Writer) -> Result<(), ProtocolError> {
        writer
            .array(3)?
            .text(self.host.as_str())?
            .uint(u64::from(self.port))
            .uint(self.idle_timeout_ms);
        Ok(())
    }

    pub(crate) fn decode(decoder: &mut Decoder<'_>) -> Result<Self, ProtocolError> {
        decoder.array_exact(3)?;
        let host = ProxyHost::new(decoder.text(MAX_PROXY_HOST_LEN)?)?;
        Self::new(host, cbor::u16_value(decoder)?, decoder.uint()?)
    }
}

/// Successful terminal summary of one proxy stream.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProxyResult {
    /// Why the stream ended.
    pub reason: EndReason,
    /// Bytes forwarded from the caller to the destination.
    pub bytes_to_target: u64,
    /// Bytes forwarded from the destination to the caller.
    pub bytes_from_target: u64,
    /// Wall-clock duration from connect to close.
    pub duration_ms: u64,
}

/// Terminal proxy outcome.
pub type ProxyOutcome = Result<ProxyResult, ServiceError>;

impl ProxyResult {
    /// Creates a summary.
    pub const fn new(reason: EndReason, bytes: (u64, u64), duration_ms: u64) -> Self {
        Self {
            reason,
            bytes_to_target: bytes.0,
            bytes_from_target: bytes.1,
            duration_ms,
        }
    }

    /// Encodes one terminal outcome frame payload.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INTERNAL`] only on allocation failure.
    pub fn encode_outcome(outcome: &ProxyOutcome) -> Result<Vec<u8>, ProtocolError> {
        cbor::encode_outcome(
            outcome.as_ref().map_err(|error| *error),
            |result, writer| {
                writer
                    .array(4)?
                    .uint(result.reason.code())
                    .uint(result.bytes_to_target)
                    .uint(result.bytes_from_target)
                    .uint(result.duration_ms);
                Ok(())
            },
        )
    }

    /// Decodes one terminal outcome frame payload.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] for malformed or noncanonical input.
    pub fn decode_outcome(bytes: &[u8]) -> Result<ProxyOutcome, ProtocolError> {
        cbor::decode_outcome(bytes, |decoder| {
            decoder.array_exact(4)?;
            let reason = EndReason::from_code(decoder.uint()?)?;
            let bytes = (decoder.uint()?, decoder.uint()?);
            Ok(Self::new(reason, bytes, decoder.uint()?))
        })
    }
}
